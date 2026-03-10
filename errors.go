package accelerator

import "errors"

var (
	ErrInvalidTTL          = errors.New("invalid ttl")
	ErrInvalidOptions      = errors.New("invalid options")
	ErrLoaderNotConfigured = errors.New("loader not configured")
	ErrCodec               = errors.New("codec error")
	ErrRemoteUnavailable   = errors.New("remote unavailable")
)
