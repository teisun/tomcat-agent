# Prompt-cache gateway handoff

> This source-repository record captures an implementation-facing cache observation. General research remains in the wiki as described by `reports/README.md`.

## Scope

The audited Tomcat build made 343 provider requests. Each later request retained the same durable
conversation prefix and appended only new assistant/tool history. The runtime tail was unchanged
throughout the build.

```
request N: [stable instructions][stable history from N-1][new tool result]
                         ^ expected reusable prefix
```

The client now emits `context_metrics_update` during the build and includes
`cacheObservation` in `plan.complete`. Treat a missing `cacheHitRatio` as “the provider did not
report cache-read usage,” not as a cache miss.

## Observation

| signal | audited build |
|---|---:|
| prompt tokens | 64.27M |
| cache-read ratio | 41% |
| zero cache-read requests | 146 / 343 |
| longest consecutive zero-read run | 24 |

The 24-request zero-read run was followed by a request that read a 251,776-token prefix. Tomcat
did not rewrite durable history or change the runtime tail during that run. That pattern is
consistent with requests reaching cache-isolated upstream workers, not with a client-side prefix
mutation on every request.

## Requested gateway checks

1. Keep requests with the same configured cache key and prefix on a cache-sharing upstream pool
   (sticky routing is sufficient; a shared prompt-cache tier is better).
2. Confirm cache keys include the complete `instructions` and ordered input prefix, but do not
   fragment by per-request transport metadata.
3. Preserve and return `prompt_tokens` plus `cache_read_tokens` for every streamed response,
   including zero reads.
4. Correlate the next recurrence using Tomcat's session ID, diagnostic request ID when enabled,
   model ID, cache key, and the new `consecutiveMissMax` metric.

## Client-side boundary

Tomcat does not change the current three provider wire layouts in this remediation. An
`EphemeralTail` is placed in the leading instructions/system area. If that tail changes, a cold
write is expected for that request; the runtime now records `tailChangedCount` and
`tailChangeMissTokens`. Reconsider that local layout only when tail changes exceed 10% of measured
requests or those requests exceed 5% of prompt tokens; apply any change per provider profile.
