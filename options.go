package accelerator

import "time"

// Options defines all cache settings in a flattened form.
// Local* configures in-process cache behavior.
// Remote* configures remote cache behavior.
type Options[K comparable, V any] struct {
	Namespace string

	Codec    Codec[V]
	KeyCodec KeyCodec[K]
	Loader   Loader[K, V]

	LocalCapacity         int
	LocalTTL              time.Duration
	LocalMissTTL          time.Duration
	LocalJitterLambda     float64
	LocalJitterUpperBound time.Duration
	LocalShards           uint64

	RemoteTTL               time.Duration
	RemoteMissTTL           time.Duration
	RemoteKeyPrefix         string
	RemoteInvalidateChannel string
}
