"""Networking + image constants for the docker-based fixture in conftest.py."""

from __future__ import annotations

NETWORK_SUBNET = "172.30.0.0/16"
BOOTSTRAP_IP = "172.30.0.10"
BOOTSTRAP_TCP_PORT = 60000
BOOTSTRAP_UDP_PORT = 60001
BOOTSTRAP_REST_PORT = 8645
NWAKU_IMAGE = "wakuorg/nwaku:v0.38.0"

# Waku cluster/shard the chat clients subscribe to. Must match bootstrap's
# --preset=logos.dev (cluster=2) + --shard=1 — pubsub topic is
# /waku/2/rs/{cluster}/{shard}, mismatch means messages don't propagate.
CHAT_CLUSTER_ID = 2
CHAT_SHARD_ID = 1

# Waku TCP ports that chat clients bind. Each container has its own network
# namespace, so values can be reused across containers without collision.
SARO_PORT = 60002
RAYA_PORT = 60003

# Distinct port for the lifecycle single-user test. Each container has its own
# netns so collision is impossible, but a separate value keeps log greps unique.
LIFECYCLE_USER_PORT = 60004
