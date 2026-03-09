package accelerator

import (
	"context"
	"time"
)

// NoExpiration indicates the value should never expire.
//
// TTL semantics:
//   - ttl == 0: no expiration
//   - ttl == -1: no expiration
//   - ttl > 0: expires after ttl
//   - ttl < -1: invalid
const NoExpiration time.Duration = -1

// Loader fetches values for missing keys.
//
// missing contains keys that are confirmed absent by the data source.
type Loader[K comparable, V any] func(ctx context.Context, keys []K) (found map[K]V, missing []K, err error)

// Codec encodes and decodes values for remote storage.
type Codec[V any] interface {
	Encode(value V) ([]byte, error)
	Decode(raw []byte) (V, error)
}

// KeyCodec encodes and decodes cache keys to a stable string representation.
type KeyCodec[K comparable] interface {
	Encode(key K) (string, error)
	Decode(raw string) (K, error)
}

// Cache defines the public cache contract for accelerator.
//
// Query methods are expected to auto-load on misses when a default Loader is configured.
// A negative/miss cache hit must suppress reloading until miss TTL expires.
type Cache[K comparable, V any] interface {
	Get(ctx context.Context, key K) (value V, found bool, err error)
	MGet(ctx context.Context, keys []K) (values map[K]V, missing []K, err error)

	Set(ctx context.Context, key K, value V, ttl time.Duration) error
	MSet(ctx context.Context, items map[K]V, ttl time.Duration) error

	Del(ctx context.Context, key K) error
	MDel(ctx context.Context, keys []K) error

	WarmUp(ctx context.Context, keys []K, ttl time.Duration) error
	Close(ctx context.Context) error
}
