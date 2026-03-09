package accelerator

import (
	"context"
	"reflect"
	"testing"
	"time"
)

func TestNoExpirationConstant(t *testing.T) {
	if NoExpiration != -1 {
		t.Fatalf("NoExpiration = %v, want -1", NoExpiration)
	}
}

func TestCacheMethodContract(t *testing.T) {
	cacheType := reflect.TypeOf((*Cache[string, int])(nil)).Elem()

	methods := map[string]struct{}{}
	for i := 0; i < cacheType.NumMethod(); i++ {
		methods[cacheType.Method(i).Name] = struct{}{}
	}

	expected := []string{
		"Get",
		"MGet",
		"Set",
		"MSet",
		"Del",
		"MDel",
		"WarmUp",
		"Close",
	}
	for _, name := range expected {
		if _, ok := methods[name]; !ok {
			t.Fatalf("expected method %s to exist", name)
		}
	}

	unexpected := []string{
		"GetMany",
		"SetMany",
		"DeleteMany",
		"MDelete",
		"SetWithTTL",
		"MSetWithTTL",
	}
	for _, name := range unexpected {
		if _, ok := methods[name]; ok {
			t.Fatalf("unexpected method %s exists", name)
		}
	}
}

func TestTTLArgumentsExistOnSetMethods(t *testing.T) {
	cacheType := reflect.TypeOf((*Cache[string, int])(nil)).Elem()

	setMethod, ok := cacheType.MethodByName("Set")
	if !ok {
		t.Fatal("Set method not found")
	}
	if got := setMethod.Type.In(3); got != reflect.TypeOf(time.Duration(0)) {
		t.Fatalf("Set ttl arg type = %v, want time.Duration", got)
	}

	msetMethod, ok := cacheType.MethodByName("MSet")
	if !ok {
		t.Fatal("MSet method not found")
	}
	if got := msetMethod.Type.In(2); got != reflect.TypeOf(time.Duration(0)) {
		t.Fatalf("MSet ttl arg type = %v, want time.Duration", got)
	}

	warmUpMethod, ok := cacheType.MethodByName("WarmUp")
	if !ok {
		t.Fatal("WarmUp method not found")
	}
	if got := warmUpMethod.Type.In(2); got != reflect.TypeOf(time.Duration(0)) {
		t.Fatalf("WarmUp ttl arg type = %v, want time.Duration", got)
	}
}

func TestBuilderChainCompiles(t *testing.T) {
	loader := func(_ context.Context, keys []string) (map[string]int, []string, error) {
		return map[string]int{}, keys, nil
	}

	configurator := func(ctx context.Context, opts *Options[string, int], next ConfigNext[string, int]) error {
		opts.Namespace = "configured"
		return next(ctx, opts)
	}

	cache, err := NewCache[string, int]().
		WithOptions(Options[string, int]{Namespace: "demo"}).
		WithLoader(loader).
		WithConfigChain(configurator).
		Build(context.Background())

	if err != ErrInvalidOptions {
		t.Fatalf("Build error = %v, want %v", err, ErrInvalidOptions)
	}
	if cache != nil {
		t.Fatal("Build cache should be nil for contract-only iteration")
	}
}

func TestBuildRunsConfiguratorChainInOrder(t *testing.T) {
	order := make([]int, 0, 2)

	first := func(ctx context.Context, opts *Options[string, int], next ConfigNext[string, int]) error {
		order = append(order, 1)
		opts.Namespace = "first"
		return next(ctx, opts)
	}
	second := func(ctx context.Context, opts *Options[string, int], next ConfigNext[string, int]) error {
		order = append(order, 2)
		if opts.Namespace != "first" {
			t.Fatalf("unexpected namespace: %s", opts.Namespace)
		}
		return next(ctx, opts)
	}

	_, err := NewCache[string, int]().
		WithConfigChain(first, second).
		Build(context.Background())

	if err != ErrInvalidOptions {
		t.Fatalf("Build error = %v, want %v", err, ErrInvalidOptions)
	}

	if len(order) != 2 || order[0] != 1 || order[1] != 2 {
		t.Fatalf("unexpected chain order: %v", order)
	}
}

func TestBuildPropagatesConfiguratorError(t *testing.T) {
	expectedErr := ErrCodec

	stop := func(_ context.Context, _ *Options[string, int], _ ConfigNext[string, int]) error {
		return expectedErr
	}

	_, err := NewCache[string, int]().
		WithConfigChain(stop).
		Build(context.Background())

	if err != expectedErr {
		t.Fatalf("Build error = %v, want %v", err, expectedErr)
	}
}
