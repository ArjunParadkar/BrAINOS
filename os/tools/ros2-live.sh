#!/usr/bin/env bash
#
# Run the ROS 2 live-graph test against a real, running ROS 2 system:
# turtlesim, headless, in a container.
#
#   1. start a ROS 2 Jazzy container (pulls ros:jazzy-ros-base, ~1.3 GB,
#      and installs turtlesim the first time)
#   2. run turtlesim_node in it with no display
#   3. run host/tests/nerves_ros2.rs with BRAINOS_NET=on and a `ros2`
#      on PATH that execs inside the container
#   4. remove the container (keep it with KEEP=1)
#
# The graph the test reads is produced by a running ROS 2 node and read
# with the real `ros2` tool. What is containerised is where ROS runs,
# not what it is.
set -euo pipefail
cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:$PATH"
if ! docker ps --format '{{.Names}}' | grep -qx brainos-ros2; then
    docker rm -f brainos-ros2 >/dev/null 2>&1 || true
    docker run -d --name brainos-ros2 ros:jazzy-ros-base sleep infinity >/dev/null
    docker exec brainos-ros2 bash -c 'apt-get update -qq >/dev/null 2>&1 && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq ros-jazzy-turtlesim >/dev/null 2>&1'
    docker exec -d brainos-ros2 bash -c 'source /opt/ros/jazzy/setup.bash && QT_QPA_PLATFORM=offscreen ros2 run turtlesim turtlesim_node >/tmp/turtle.log 2>&1'
    sleep 6
fi
status=0
PATH="$PWD/tools/ros2-in-docker:$PATH" BRAINOS_NET=on \
    cargo test --offline --release -p brainos-host --test nerves_ros2 -- --nocapture live_ || status=$?
[ "${KEEP:-0}" = 1 ] || docker rm -f brainos-ros2 >/dev/null 2>&1 || true
exit $status
