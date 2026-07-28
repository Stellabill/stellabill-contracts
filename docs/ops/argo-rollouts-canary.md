# Argo Rollouts canary strategy — operations guide

This document covers the day-to-day operation of the stellabill-backend
canary rollout: how it works, how to observe it, how to manually
pause/resume, and what happens on analysis failure.

---

## Table of contents

- [How the rollout works](#how-the-rollout-works)
- [Prerequisites](#prerequisites)
- [Dry-run and inspection](#dry-run-and-inspection)
- [Triggering a rollout](#triggering-a-rollout)
- [Monitoring progress](#monitoring-progress)
- [Manual pause and resume](#manual-pause-and-resume)
- [Automatic rollback on analysis failure](#automatic-rollback-on-analysis-failure)
- [Forcing a rollback manually](#forcing-a-rollback-manually)
- [Analysis details](#analysis-details)
- [Tuning thresholds](#tuning-thresholds)
- [Edge cases and safety invariants](#edge-cases-and-safety-invariants)
- [Troubleshooting](#troubleshooting)

---

## How the rollout works

Traffic shifts in four increments, each gated by automated analysis:

```
stable (100%)
    │
    ▼  new image deployed
[10% canary] ──► AnalysisRun ──► pass → [25%] ──► AnalysisRun ──► pass
    → [50%] ──► AnalysisRun ──► pass → [100% promoted — rollout complete]
                                     ↓ fail (any step)
                              automatic abort → revert to stable
```

Each `AnalysisRun` evaluates three Prometheus metrics over a configurable
window (default: 120 s):

| Metric | Passes when |
|--------|------------|
| `p99-latency` | p99 < `latencyP99ThresholdSeconds` (default 500 ms) |
| `error-rate-5xx` | 5xx fraction < `errorRateThreshold` (default 1 %) |
| `min-traffic-guard` | total requests ≥ `minSampleCount` (default 10) |

If **any** metric fails, the controller immediately aborts the rollout and
restores the stable revision without any human action.

---

## Prerequisites

```bash
# Argo Rollouts kubectl plugin
kubectl argo rollouts version

# If not installed:
curl -LO https://github.com/argoproj/argo-rollouts/releases/latest/download/kubectl-argo-rollouts-linux-amd64
chmod +x kubectl-argo-rollouts-linux-amd64
sudo mv kubectl-argo-rollouts-linux-amd64 /usr/local/bin/kubectl-argo-rollouts
```

---

## Dry-run and inspection

Render the chart locally without applying it:

```bash
helm template stellabill deploy/helm/stellabill \
  --namespace stellabill \
  --set image.tag=v1.2.3 \
  | grep -A 5 'kind: Rollout'
```

Inspect the live rollout (no cluster changes):

```bash
kubectl argo rollouts get rollout stellabill \
  --namespace stellabill \
  --watch=false
```

Expected output while a rollout is in progress:

```
Name:            stellabill
Namespace:       stellabill
Status:          ॥ Paused
Message:         CanaryPauseStep
Strategy:        Canary
  Step:          3/7
  SetWeight:     25
  ActualWeight:  25
Images:
  ghcr.io/stellabill/stellabill-backend:v1.1.0 (stable)
  ghcr.io/stellabill/stellabill-backend:v1.2.3 (canary, weight: 25)
Replicas:
  Desired:       3
  Current:       4
  Updated:       1
  Ready:         4
  Available:     4
```

---

## Triggering a rollout

A rollout is triggered automatically when the Rollout's pod template changes
(e.g. a new image tag pushed by CI).  You can also trigger manually:

```bash
# Bump the image tag (standard CD path)
helm upgrade stellabill deploy/helm/stellabill \
  --namespace stellabill \
  --set image.tag=v1.2.3 \
  --atomic \
  --timeout 10m

# Or restart with the current image (re-runs analysis):
kubectl argo rollouts restart rollout stellabill --namespace stellabill
```

---

## Monitoring progress

```bash
# Live watch — updates in the terminal as steps complete
kubectl argo rollouts get rollout stellabill \
  --namespace stellabill \
  --watch

# List active AnalysisRuns for this rollout
kubectl get analysisrun \
  --namespace stellabill \
  -l rollouts-pod-template-hash

# Inspect a specific AnalysisRun
kubectl describe analysisrun <name> --namespace stellabill
```

Prometheus queries you can run manually to cross-check:

```promql
# p99 latency over the last 2 minutes for the canary service
histogram_quantile(
  0.99,
  sum(rate(http_request_duration_seconds_bucket{service="stellabill-canary"}[2m])) by (le)
)

# 5xx error rate
sum(rate(http_requests_total{service="stellabill-canary", status=~"5.."}[2m]))
/
sum(rate(http_requests_total{service="stellabill-canary"}[2m]))
```

---

## Manual pause and resume

You may need to pause a rollout temporarily (e.g. to coordinate a database
migration, wait for a dependent service, or investigate a warning that hasn't
breached the analysis threshold yet).

### Pause

```bash
kubectl argo rollouts pause rollout stellabill --namespace stellabill
```

The rollout halts at the current step.  No traffic shift occurs while paused.
Any in-flight AnalysisRun is suspended.

### Resume

```bash
kubectl argo rollouts resume rollout stellabill --namespace stellabill
```

The rollout continues from where it was paused, re-running any analysis that
was interrupted.

### Promote past a step (skip current analysis)

Use this only in emergencies after consulting the team:

```bash
# Promote one step:
kubectl argo rollouts promote rollout stellabill --namespace stellabill

# Promote all remaining steps (full promotion, skips all analysis):
kubectl argo rollouts promote rollout stellabill --namespace stellabill --full
```

> **Warning:** `--full` bypasses all remaining analysis steps.  Only use
> when you have out-of-band confirmation that the new revision is healthy
> (e.g. a confirmed incident fix with manual verification).

---

## Automatic rollback on analysis failure

When an `AnalysisRun` produces a `Failed` result the controller:

1. Sets the rollout status to `Degraded`.
2. Scales down all canary pods.
3. Restores the stable Service selector to the previous revision.
4. Leaves a failed `AnalysisRun` object in the namespace for post-mortem.

**No manual intervention is required for rollback.**

You will see in `kubectl argo rollouts get rollout stellabill`:

```
Status:   ✖ Degraded
Message:  RolloutAborted: metric "p99-latency" assessed Failed due to failed
          (1) > failureLimit (0)
```

After investigating, to re-attempt with the same image (e.g. after fixing a
Prometheus query or correcting a threshold):

```bash
kubectl argo rollouts retry rollout stellabill --namespace stellabill
```

---

## Forcing a rollback manually

If you need to immediately roll back the stable revision to a previous image:

```bash
# Roll back to the immediately previous revision
kubectl argo rollouts undo rollout stellabill --namespace stellabill

# Roll back to a specific revision number
kubectl argo rollouts undo rollout stellabill \
  --namespace stellabill \
  --to-revision=3
```

Check revision history:

```bash
kubectl argo rollouts history rollout stellabill --namespace stellabill
```

---

## Analysis details

The `AnalysisTemplate` is defined in
`deploy/helm/stellabill/templates/analysis-template.yaml`.

Three metrics are evaluated at every analysis step:

### `p99-latency`

Queries `histogram_quantile(0.99, ...)` over the `http_request_duration_seconds_bucket`
metric for the canary service.  Fails if the 99th-percentile latency exceeds
the configured threshold.

`failureLimit: 0` — a single failure aborts the rollout.
`consecutiveErrorLimit: 2` — two consecutive Prometheus query errors
(e.g. connectivity issues) also abort.

### `error-rate-5xx`

Computes the ratio of `http_requests_total{status=~"5.."}` to
`http_requests_total` for the canary service.  Fails if the ratio exceeds
the configured threshold.

Same limits as above.

### `min-traffic-guard`

Counts total requests received by the canary service in the analysis window.
If this count is below `minSampleCount` (default: 10) the metric fails,
aborting the rollout.

This is the **zero-sample safety guard**: if the canary receives no traffic
(due to a misconfigured service, a routing bug, or a Prometheus scrape
failure) the analysis aborts rather than silently passing.

---

## Tuning thresholds

Override defaults per-environment with a values override file:

```yaml
# values.production.yaml
analysis:
  latencyP99ThresholdSeconds: "0.3"   # tighter: 300 ms
  errorRateThreshold: "0.005"          # tighter: 0.5 %
  minSampleCount: "50"                 # require more traffic before passing
  durationSeconds: 300                 # longer window: 5 minutes
```

```bash
helm upgrade stellabill deploy/helm/stellabill \
  --namespace stellabill \
  -f values.production.yaml \
  --set image.tag=v1.2.3
```

---

## Edge cases and safety invariants

| Scenario | Behaviour |
|----------|-----------|
| Prometheus unreachable during analysis | Query returns error → `consecutiveErrorLimit` reached → abort |
| Canary receives zero requests | `min-traffic-guard` fails → abort |
| Analysis window too short for statistical significance | Increase `durationSeconds` and `minSampleCount` |
| Both stable and canary fail health checks | Pod disruption budget prevents full outage; rollout degrades |
| Rollout stuck mid-step > `progressDeadlineSeconds` | Controller marks rollout `Degraded`; stable traffic unaffected |
| Manual `--full` promotion with failing canary | Operator bypasses analysis; stable traffic may degrade — use only as a last resort |

---

## Troubleshooting

**Rollout stuck in `Paused` with no active analysis:**

```bash
# Check if it was manually paused
kubectl argo rollouts get rollout stellabill --namespace stellabill
# If Status: Paused, resume it:
kubectl argo rollouts resume rollout stellabill --namespace stellabill
```

**AnalysisRun shows `Error` but not `Failed`:**

```bash
kubectl describe analysisrun <name> --namespace stellabill
# Look for Prometheus connectivity errors in the Events section.
# Verify Prometheus is reachable from inside the cluster:
kubectl run -it --rm debug --image=curlimages/curl --restart=Never \
  -- curl -s http://prometheus-operated.monitoring.svc.cluster.local:9090/-/healthy
```

**`min-traffic-guard` always fails in staging (low traffic):**

Lower `minSampleCount` for the staging environment:

```yaml
# values.staging.yaml
analysis:
  minSampleCount: "3"
  durationSeconds: 60
```

**How to check which revision is currently stable:**

```bash
kubectl argo rollouts get rollout stellabill --namespace stellabill \
  | grep '(stable)'
```
