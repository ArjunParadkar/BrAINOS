# blur/hpcc — BLUR training on AHPCC Pinnacle (Track 4)

The same `blur/` tree runs on the laptop and on Pinnacle. The two differ only
in a site file (`site/laptop.env`, `site/pinnacle.env`): where the MJCF copy,
runs and logs live, and how python is started (`bpy`).

## Pieces

| file | what |
|---|---|
| `../blurcfg.py` | the per-machine layer every BLUR script loads the descriptor through. `BLUR_MJCF_DIR` overrides the descriptor's laptop path; `blur.xml` must hash to the descriptor's `mjcf_sha256` or the process exits (a stale/edited copy can't train silently) |
| `site/pinnacle.env`, `site/laptop.env` | `BLUR_HOME`, `BLUR_CODE`, `BLUR_MJCF_DIR`, `BLUR_SIF`, `BLUR_RUNS`, `BLUR_LOGS`, `BLUR_PY`, thread counts |
| `bpy` | "python" on any site. If `BLUR_SIF` is set: `singularity exec [--nv on a GPU node] --cleanenv -B /scrfs $BLUR_SIF`, passing through `BLUR_*`/`OMP_*`/`XLA_*`/`JAX_*`/`CUDA_VISIBLE_DEVICES`. If not, the laptop venv |
| `blur.def`, `requirements.txt` | the image: `python:3.12-slim-bookworm` + pinned mujoco 3.16.0 / mujoco-mjx 3.16.0 / mujoco-warp 3.16.0 / jax[cuda12] 0.12.0.1 / numpy 2.5.4 / scipy 1.18.1 / cma / imageio / torch 2.14.1+cpu (the laptop venv's versions) |
| `jobs/build_image.sbatch` | builds the image in a cloud72 job. Pinnacle has no `/etc/subuid` entry for us, so `--fakeroot` fails; the job falls back to an unprivileged route: docker → sandbox on `/local_scratch/$SLURM_JOB_ID` → `pip install` inside `--writable` → SIF. apt is skipped on that route (no EGL/OSMesa in the image; nothing renders on the cluster, and `bpy` sets `MUJOCO_GL=disable`) |
| `sync_to_pinnacle.sh` | (laptop) tar-over-ssh copy of `blur/` → `$BLUR_HOME/code/blur` (no runs/logs/schema data) and the v36 MJCF → `$BLUR_HOME/mjcf/blur_mujoco_v36`. The laptop MJCF is only read |
| `smoke.py` + `jobs/smoke_{cpu,gpu}.sbatch` | imports + steps everything: versions, descriptor sha, full MJCF 1 s on CPU MuJoCo, `env.BlurEnv` + torch MLP 25 policy steps, MJX 64 envs × 100 steps |
| `mjx_parity.py` + `jobs/mjx_4c_a100.sbatch` | 4c: MJX vs CPU MuJoCo on fixed rollouts (below) |
| `mjx_throughput.py` (+ `jobs/mjx_4c_v100.sbatch`, `jobs/mjx_4c_a100_1k.sbatch`) | 4c: MJX env-steps/s at 1k/4k/8k envs |
| `jobs/parity_cpu_check.sbatch` | 2 %-length MJX-on-CPU run of the 4c scripts (cloud72): catches code errors before a GPU job |
| `site/snapshot.sh` | sourced by every job: runs it from `$BLUR_HOME/snap/$SLURM_JOB_ID`, a copy of the code, so a sync can't break a running job (it did once: job 1367539) |

Image: `/scrfs/storage/paradkar/blur/images/blur-mjx-0.12-mj3.16.sif`
(4.0 GB; `pip freeze` in `<sif>.freeze.txt`, sha256 in `<sif>.sha256`).

## How to run

```sh
# laptop: push the code + MJCF (needs the open ControlMaster: uark-vpn up; ssh pinnacle)
blur/hpcc/sync_to_pinnacle.sh
# Pinnacle login node: submit (never run python there)
cd /scrfs/storage/paradkar/blur/code/blur/hpcc
sbatch --export=NONE jobs/build_image.sbatch      # once per requirements change (~11 min, cloud72)
sbatch --export=NONE jobs/smoke_cpu.sbatch
sbatch --export=NONE jobs/smoke_gpu.sbatch        # check the account's gpu cores first (≤ 168 running)
# inside any job script:
source /scrfs/storage/paradkar/blur/code/blur/hpcc/site/pinnacle.env
"$BLUR_PY" ../learn/ppo.py --task standup ...     # same command line as the laptop
```

On the laptop nothing changes (`blurcfg` falls back to the descriptor path;
`train_gate2.sh`/`after_training.sh` take `BLUR_PY`, default the venv).

Rules (Arjun): job names `blur-*`; `--export=NONE`; ≤ 1 blur GPU job at a
time and only while the account's running gpu-QOS cores ≤ 168; no python on
the login node; no large allocations until Arjun has seen the numbers.

## Training campaigns (4d)

```sh
# laptop: sync, then on Pinnacle freeze the code + manifest for a new campaign
blur/hpcc/sync_to_pinnacle.sh
ssh pinnacle bash /scrfs/storage/paradkar/blur/code/blur/hpcc/campaign_setup.sh dayone
# start N lanes (check: account running comp cores <= 96 first)
for L in 1 2; do ssh pinnacle sbatch --export=NONE,CAMPAIGN=dayone -J blur-lane-$L -p comp06 -q comp -c 32 \
  -t 6:00:00 --dependency=singleton /scrfs/storage/paradkar/blur/campaigns/dayone/code/hpcc/jobs/lane.sbatch; done
# laptop: results + ranked table
blur/hpcc/fetch_runs.sh dayone      # -> blur/learn/runs/hpcc_dayone/SUMMARY.md
# stop: touch /scrfs/storage/paradkar/blur/campaigns/dayone/STOP  (lanes finish their segment, don't resubmit)
```

| file | what |
|---|---|
| `campaigns/<name>/{manifest.txt,campaign.env}` | runs (`id task seed hours`; `ZERO:<skill>` = reference baseline) and settings (`BLUR_DR`, workers, eval size) |
| `campaign_setup.sh` | freezes the synced code as `campaigns/<name>/code` (+ hash); refuses to overwrite a live campaign |
| `jobs/lane.sbatch` | singleton lane: lock-claim the next unfinished run, comp-core guard (≤ 96; squeue failure = busy), node-hour cap (`CAP_NODE_H`, default 210), self-resubmission |
| `run_one.sh` | one run: train (`ppo.py --auto-resume --deadline`, exit 75 = resume later) → deterministic eval nominal + stress → `DONE` |
| `jobs/resume_test.sbatch` | bitwise resume test (PASS, job 1368258) |
| `fetch_runs.sh`, `summarize.py` | results back to `blur/learn/runs/hpcc_<name>/` (no ckpt/.pt) and the ranked `SUMMARY.md` |

Campaign directories: `/scrfs/storage/paradkar/blur/campaigns/<name>/{code,runs,state,CODE_SHA}`.

## Results (2026-10-24)

- 4b: CPU smoke PASS (job 1367526, CentOS 7 node), GPU smoke PASS (job 1367520, A100, driver 550).
- 4c: on the same model, MJX matches CPU MuJoCo to ≤ 0.06 rad over 5 s of walking or a 3 s fall, in f32 or f64; no run crossed 0.1 rad. The training-model vs full-MJCF gap is 3–20× larger. MJX throughput on the training model is ~23k env-steps/s on an A100 (~10k on a V100), about 10× slower than the 24 CPU cores the same job reserves. Paws-only collisions reach 177k/99k but can't simulate a fall.


See `docs/BLUR_BRINGUP.md`, section "Track 4". Raw JSON: `$BLUR_RUNS/4c/<jobid>/`
on Pinnacle, copied to `blur/hpcc/results/4c/`.
