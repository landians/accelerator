package accelerator

import "context"

// ConfigNext points to the next configurator in the chain.
type ConfigNext[K comparable, V any] func(ctx context.Context, opts *Options[K, V]) error

// Configurator applies config mutations and may delegate to the next step.
// It can short-circuit the chain by returning without calling next.
type Configurator[K comparable, V any] func(ctx context.Context, opts *Options[K, V], next ConfigNext[K, V]) error

// Builder configures and builds a cache instance.
type Builder[K comparable, V any] struct {
	opts          Options[K, V]
	configurators []Configurator[K, V]
}

// NewCache starts cache construction with fluent builder syntax.
func NewCache[K comparable, V any]() *Builder[K, V] {
	return &Builder[K, V]{}
}

func (b *Builder[K, V]) WithOptions(opts Options[K, V]) *Builder[K, V] {
	b.opts = opts
	return b
}

func (b *Builder[K, V]) WithLoader(loader Loader[K, V]) *Builder[K, V] {
	b.opts.Loader = loader
	return b
}

// WithConfigChain appends configurators using chain-of-responsibility style.
// Configurators run in registration order.
func (b *Builder[K, V]) WithConfigChain(configurators ...Configurator[K, V]) *Builder[K, V] {
	for _, configurator := range configurators {
		if configurator == nil {
			continue
		}
		b.configurators = append(b.configurators, configurator)
	}
	return b
}

func (b *Builder[K, V]) Build(ctx context.Context) (Cache[K, V], error) {
	if err := b.applyConfigChain(ctx); err != nil {
		return nil, err
	}

	return nil, ErrInvalidOptions
}

func (b *Builder[K, V]) applyConfigChain(ctx context.Context) error {
	next := ConfigNext[K, V](func(_ context.Context, _ *Options[K, V]) error {
		return nil
	})

	for i := len(b.configurators) - 1; i >= 0; i-- {
		configurator := b.configurators[i]
		prevNext := next

		next = func(ctx context.Context, opts *Options[K, V]) error {
			return configurator(ctx, opts, prevNext)
		}
	}

	opts := b.opts
	if err := next(ctx, &opts); err != nil {
		return err
	}

	b.opts = opts
	return nil
}
