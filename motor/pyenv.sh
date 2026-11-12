# The Python the motor tooling runs under: MuJoCo 3 + numpy (+ torch).
# Default is the BLUR twin's venv on the laptop; set MOTOR_PY elsewhere.
export MOTOR_PY="${MOTOR_PY:-$HOME/Desktop/Arenda_Projects/BLUR/blur_mujoco/.venv/bin/python}"
export BRAINOS_BIN="${BRAINOS_BIN:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/os/target/release/brainos}"
