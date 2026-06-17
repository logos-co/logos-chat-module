"""Three-container fixture: nwaku-bootstrap + Saro + Raya in shared docker network.

Why bootstrap-node: liblogoschat's `staticPeers` accepts only ENRs (not
multiaddrs), and `chat_get_id` returns the configured installation_name (not
a libp2p peerId), so two instances can't introduce themselves to each other
from configJson alone. A third nwaku container with a deterministic ENR
acts as the rendezvous; gossipsub/relay-mesh handles the rest.

The skip-cascade in `_e2e_env_or_skip` duplicates the framework's own
`local_daemon` / `docker_daemon` skip logic by design — we need TWO
chat-clients in a SHARED network, the framework provides only single-instance
fixtures.

Naming follows `logos-messaging/specs:informational/chat_cast.md` (Saro = sender,
Raya = recipient, Pax = additional participant).
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import time
import urllib.error
import urllib.request
import uuid
from collections.abc import Iterator
from contextlib import ExitStack
from pathlib import Path
from typing import TYPE_CHECKING

import pytest

from libs.constants import (
    BOOTSTRAP_IP,
    BOOTSTRAP_REST_PORT,
    BOOTSTRAP_TCP_PORT,
    BOOTSTRAP_UDP_PORT,
    NETWORK_SUBNET,
    NWAKU_IMAGE,
    RAYA_PORT,
    SARO_PORT,
)
from libs.helpers import (
    BareChatClientFactory,
    ChatUser,
    ChatUserFactory,
    make_chat_config,
    setup_chat_user,
)

if TYPE_CHECKING:
    from logoscore import LogoscoreClient

FIXTURES_DIR = Path(__file__).parent / "fixtures"


def _read_fixture(name: str) -> str:
    return (FIXTURES_DIR / name).read_text().strip()


def _docker_logs(container_name: str) -> str:
    r = subprocess.run(
        ["docker", "logs", container_name],
        capture_output=True, text=True,
    )
    return (r.stdout or "") + (r.stderr or "")


def _save_logs(container_name: str) -> None:
    log_dir = Path(os.environ.get("E2E_LOG_DIR", "/tmp"))
    log_dir.mkdir(parents=True, exist_ok=True)
    try:
        (log_dir / f"{container_name}.log").write_text(_docker_logs(container_name))
    except OSError as e:
        sys.stderr.write(f"warning: failed to save log for {container_name}: {e}\n")


@pytest.fixture(scope="session")
def _e2e_env_or_skip() -> tuple[str, Path]:
    """Single gate: skip if any prerequisite (docker, image, modules layout) is missing."""
    from logoscore import docker_available, image_present  # noqa: PLC0415

    if not docker_available():
        pytest.skip("docker not available")
    image = os.environ.get("LOGOSCORE_IMAGE")
    if not image:
        pytest.skip("LOGOSCORE_IMAGE not set")
    if not image_present(image):
        pytest.skip(f"LOGOSCORE_IMAGE={image!r} not present locally")
    modules_dir_env = os.environ.get("LOGOS_MODULES_DIR")
    if not modules_dir_env:
        pytest.skip("LOGOS_MODULES_DIR not set")
    modules_dir = Path(modules_dir_env)
    if not modules_dir.is_dir():
        pytest.skip(f"LOGOS_MODULES_DIR={modules_dir_env!r} is not a directory")
    if not (modules_dir / "chat_module" / "manifest.json").is_file():
        pytest.skip(
            f"LOGOS_MODULES_DIR={modules_dir} doesn't contain "
            "chat_module/manifest.json — did you run `nix build .#install-portable`?"
        )
    return image, modules_dir


@pytest.fixture(scope="session")
def shared_docker_network(
    _e2e_env_or_skip: tuple[str, Path],
) -> Iterator[str]:
    name = f"logoschat-e2e-{uuid.uuid4().hex[:8]}"
    r = subprocess.run(
        ["docker", "network", "create", "--subnet", NETWORK_SUBNET, name],
        capture_output=True, text=True,
    )
    if r.returncode != 0:
        # Fail loud rather than skip — a CI runner that can't create a docker
        # network silently passing the job would mask a real environment break.
        pytest.fail(f"failed to create docker network {name!r}: {r.stderr.strip()}")
    try:
        yield name
    finally:
        subprocess.run(["docker", "network", "rm", name], capture_output=True)


@pytest.fixture(scope="session")
def nwaku_bootstrap(shared_docker_network: str) -> Iterator[str]:
    """Spin up nwaku and yield its live ENR.

    ENR is read from REST `/debug/v1/info` per run rather than committed —
    the nodekey is fixed (deterministic peerId) but the ENR encoding can
    shift between nwaku versions, and a stale committed ENR would silently
    misroute peers. REST is published on 127.0.0.1:<random_host_port> so
    we can hit it via stdlib urllib (no http tools inside the image).
    """
    from logoscore import pick_free_port  # noqa: PLC0415

    nodekey = _read_fixture("bootstrap-nodekey.txt")
    container_name = f"nwaku-bootstrap-{uuid.uuid4().hex[:8]}"
    rest_host_port = pick_free_port()

    cmd = [
        "docker", "run", "-d", "--name", container_name,
        "--network", shared_docker_network, "--ip", BOOTSTRAP_IP,
        "-p", f"127.0.0.1:{rest_host_port}:{BOOTSTRAP_REST_PORT}",
        NWAKU_IMAGE,
        "--preset=logos.dev", "--shard=1",
        "--filter=true", "--lightpush=true",
        f"--nodekey={nodekey}",
        f"--tcp-port={BOOTSTRAP_TCP_PORT}",
        f"--nat=extip:{BOOTSTRAP_IP}",
        "--discv5-discovery=true",
        f"--discv5-udp-port={BOOTSTRAP_UDP_PORT}",
        "--rest=true", "--rest-address=0.0.0.0", f"--rest-port={BOOTSTRAP_REST_PORT}",
        "--log-level=INFO",
    ]
    r = subprocess.run(cmd, capture_output=True, text=True)
    if r.returncode != 0:
        pytest.fail(f"failed to start nwaku-bootstrap: {r.stderr.strip()}")

    try:
        deadline = time.time() + 30.0
        last_err = None
        while time.time() < deadline:
            try:
                with urllib.request.urlopen(
                    f"http://127.0.0.1:{rest_host_port}/debug/v1/info",
                    timeout=2.0,
                ) as resp:
                    info = json.loads(resp.read().decode("utf-8"))
                    yield info["enrUri"]
                    return
            except (urllib.error.URLError, ConnectionError, json.JSONDecodeError) as e:
                last_err = e
                time.sleep(0.5)
        pytest.fail(
            f"nwaku-bootstrap REST did not become ready within 30s; "
            f"last error: {last_err!r}\n"
            f"docker logs:\n{_docker_logs(container_name)}"
        )
    finally:
        _save_logs(container_name)
        subprocess.run(["docker", "rm", "-f", container_name], capture_output=True)


@pytest.fixture(scope="module")
def chat_user_factory(
    _e2e_env_or_skip: tuple[str, Path],
    shared_docker_network: str,
    nwaku_bootstrap: str,
) -> Iterator[ChatUserFactory]:
    """Factory for ChatUser instances. Each call spawns a LogoscoreDockerDaemon
    container in the shared network and returns an initialised ChatUser.

    All daemon teardown + log capture is registered on a single ExitStack
    that's closed when the module's tests finish, so any number of users can
    be created in a single test session without leaking containers.
    """
    image, modules_dir = _e2e_env_or_skip

    # Lazy import — keeps `pytest collect` working when the framework isn't installed.
    from logoscore import LogoscoreDockerDaemon  # noqa: PLC0415

    with ExitStack() as stack:
        def _create(name: str, port: int) -> ChatUser:
            container_name = f"logoscore-{name.lower()}-{uuid.uuid4().hex[:8]}"
            daemon = stack.enter_context(LogoscoreDockerDaemon(
                image=image, modules_dir=modules_dir,
                container_name=container_name,
                network=shared_docker_network,
                startup_timeout=60.0,           # cold-start liblogoschat can take 30s+
                # `--verbose` flips logoscore's qInstallMessageHandler to forward
                # qDebug/qInfo/qWarning to stderr (else suppressed). We need this
                # to see the SDK's RPC trace inside the daemon when investigating
                # why a `call` returns METHOD_FAILED — without it docker logs
                # only contain the plugin's own fprintf lines.
                extra_args=["--verbose"],
            ))
            # stack.callback runs LIFO before daemon teardown, so logs are
            # captured even when the test fails after start.
            stack.callback(_save_logs, container_name)
            client = daemon.client()
            config = make_chat_config(name=name, port=port, bootstrap_enr=nwaku_bootstrap)
            return setup_chat_user(client, config_json=config, label=name)

        yield _create


@pytest.fixture(scope="module")
def saro(chat_user_factory: ChatUserFactory) -> ChatUser:
    return chat_user_factory("Saro", SARO_PORT)


@pytest.fixture(scope="module")
def raya(chat_user_factory: ChatUserFactory) -> ChatUser:
    return chat_user_factory("Raya", RAYA_PORT)


@pytest.fixture(scope="function")
def bare_chat_client_factory(
    _e2e_env_or_skip: tuple[str, Path],
    shared_docker_network: str,
) -> Iterator[BareChatClientFactory]:
    """Factory for zero-init clients: spawns a daemon and loads chat_module,
    but does NOT call initChat. For negative-path tests of sync-False branches.

    Function-scope: a daemon poisoned by an intentional bad initChat must not
    leak into the next negative test. Doesn't need `nwaku_bootstrap` since
    without initChat there's no waku binding.
    """
    image, modules_dir = _e2e_env_or_skip

    from logoscore import LogoscoreDockerDaemon  # noqa: PLC0415

    with ExitStack() as stack:
        def _create(name: str) -> LogoscoreClient:
            container_name = f"logoscore-{name.lower()}-{uuid.uuid4().hex[:8]}"
            daemon = stack.enter_context(LogoscoreDockerDaemon(
                image=image, modules_dir=modules_dir,
                container_name=container_name,
                network=shared_docker_network,
                startup_timeout=60.0,
                extra_args=["--verbose"],
            ))
            stack.callback(_save_logs, container_name)
            client = daemon.client()
            client.load_module("chat_module")
            return client

        yield _create
