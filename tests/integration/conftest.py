"""Three-container fixture: nwaku-bootstrap + Alice + Bob in shared docker network.

Why bootstrap-node: liblogoschat's `staticPeers` accepts only ENRs (not
multiaddrs), and `chat_get_id` returns the configured installation_name (not
a libp2p peerId), so two instances can't introduce themselves to each other
from configJson alone. A third nwaku container with a deterministic ENR
acts as the rendezvous; gossipsub/relay-mesh handles the rest.

The skip-cascade in `_integration_env_or_skip` duplicates the framework's own
`local_daemon` / `docker_daemon` skip logic by design — we need TWO
chat-clients in a SHARED network, the framework provides only single-instance
fixtures.
"""

from __future__ import annotations

import json
import os
import subprocess
import time
import urllib.error
import urllib.request
import uuid
from collections.abc import Iterator
from contextlib import ExitStack
from pathlib import Path

import pytest

from _helpers import ChatUser, setup_chat_user

FIXTURES_DIR = Path(__file__).parent / "fixtures"
NETWORK_SUBNET = "172.30.0.0/16"
BOOTSTRAP_IP = "172.30.0.10"
BOOTSTRAP_TCP_PORT = 60000
BOOTSTRAP_UDP_PORT = 60001
BOOTSTRAP_REST_PORT = 8645
NWAKU_IMAGE = "wakuorg/nwaku:v0.38.0"


def _read_fixture(name: str) -> str:
    return (FIXTURES_DIR / name).read_text().strip()


def _docker_logs(container_name: str) -> str:
    r = subprocess.run(
        ["docker", "logs", container_name],
        capture_output=True, text=True,
    )
    return (r.stdout or "") + (r.stderr or "")


def _save_logs(container_name: str) -> None:
    log_dir = Path(os.environ.get("INTEGRATION_LOG_DIR", "/tmp"))
    log_dir.mkdir(parents=True, exist_ok=True)
    try:
        (log_dir / f"{container_name}.log").write_text(_docker_logs(container_name))
    except OSError:
        pass


@pytest.fixture(scope="session")
def _integration_env_or_skip() -> tuple[str, Path]:
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
    _integration_env_or_skip: tuple[str, Path],
) -> Iterator[str]:
    name = f"logoschat-it-{uuid.uuid4().hex[:8]}"
    r = subprocess.run(
        ["docker", "network", "create", "--subnet", NETWORK_SUBNET, name],
        capture_output=True, text=True,
    )
    if r.returncode != 0:
        # Subnet collision / docker overload: infrastructure issue, not a test fail.
        pytest.skip(f"failed to create docker network {name!r}: {r.stderr.strip()}")
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
def chat_users(
    _integration_env_or_skip: tuple[str, Path],
    shared_docker_network: str,
    nwaku_bootstrap: str,
) -> Iterator[tuple[ChatUser, ChatUser]]:
    """Two LogoscoreDockerDaemon containers in the shared network with chat_module
    initialised + started, each pointing at nwaku_bootstrap's ENR.

    Module-scope: spinning up two daemons + waku-stack init costs ~30-60s.
    """
    image, modules_dir = _integration_env_or_skip

    # Lazy import — keeps `pytest collect` working when the framework isn't installed.
    from logoscore import LogoscoreDockerDaemon  # noqa: PLC0415

    config_a = _read_fixture("chat-a.json").replace("__BOOTSTRAP_ENR__", nwaku_bootstrap)
    config_b = _read_fixture("chat-b.json").replace("__BOOTSTRAP_ENR__", nwaku_bootstrap)

    alice_name = f"logoscore-alice-{uuid.uuid4().hex[:8]}"
    bob_name = f"logoscore-bob-{uuid.uuid4().hex[:8]}"

    with ExitStack() as stack:
        daemon_a = stack.enter_context(LogoscoreDockerDaemon(
            image=image, modules_dir=modules_dir,
            container_name=alice_name,
            network=shared_docker_network,
            startup_timeout=60.0,           # cold-start liblogoschat can take 30s+
        ))
        daemon_b = stack.enter_context(LogoscoreDockerDaemon(
            image=image, modules_dir=modules_dir,
            container_name=bob_name,
            network=shared_docker_network,
            startup_timeout=60.0,
        ))
        # stack.callback runs LIFO before daemon teardown,
        # so logs are captured even when the test fails after start.
        stack.callback(_save_logs, bob_name)
        stack.callback(_save_logs, alice_name)

        client_a = daemon_a.client()
        client_b = daemon_b.client()
        user_a = setup_chat_user(client_a, config_json=config_a, label="A")
        user_b = setup_chat_user(client_b, config_json=config_b, label="B")
        yield (user_a, user_b)
