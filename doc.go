// Package accelerator defines the public API contract for a multi-level cache.
//
// This iteration only provides interfaces and type definitions.
// Runtime behavior is planned for the next iteration.
//
// Expected behavior contract for the implementation phase:
//   - Get/MGet should auto-load on cache misses when a Loader is configured.
//   - Miss cache hits should suppress reload until miss TTL expires.
//   - ttl == 0 and ttl == -1 both mean no expiration.
//   - ttl > 0 means expiring entries.
//   - ttl < -1 is invalid and should return ErrInvalidTTL.
package accelerator
