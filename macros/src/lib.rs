use proc_macro::TokenStream;

use proc_macro_crate::{FoundCrate, crate_name};
use quote::{format_ident, quote};
use syn::parse::Parser;
use syn::punctuated::Punctuated;
use syn::{
    Error, Expr, ExprLit, FnArg, GenericArgument, ItemFn, Lit, Meta, MetaNameValue, PathArguments,
    ReturnType, Signature, Token, Type, TypePath, parse_macro_input, spanned::Spanned,
};

/// Error handling policy for cache operation failures.
#[derive(Clone, Copy, PartialEq, Eq)]
enum OnCacheError {
    /// Ignore cache errors and keep business flow running.
    Ignore,
    /// Return cache errors to caller immediately.
    Propagate,
}

/// Parsed arguments for `#[cacheable(...)]`.
struct CacheableArgs {
    cache: Expr,
    key: Expr,
    allow_stale: bool,
    cache_none: bool,
    on_cache_error: OnCacheError,
}

/// Parsed arguments for `#[cache_put(...)]`.
struct CachePutArgs {
    cache: Expr,
    key: Expr,
    value: Expr,
    on_cache_error: OnCacheError,
}

/// Parsed arguments for `#[cache_evict(...)]`.
struct CacheEvictArgs {
    cache: Expr,
    key: Expr,
    before: bool,
    on_cache_error: OnCacheError,
}

/// Parsed arguments for `#[cacheable_batch(...)]`.
struct CacheableBatchArgs {
    cache: Expr,
    keys: Expr,
    allow_stale: bool,
    on_cache_error: OnCacheError,
}

/// Parsed arguments for `#[cache_evict_batch(...)]`.
struct CacheEvictBatchArgs {
    cache: Expr,
    keys: Expr,
    before: bool,
    on_cache_error: OnCacheError,
}

/// Cache-first wrapper: hit returns directly, miss executes function and backfills cache.
#[proc_macro_attribute]
pub fn cacheable(attr: TokenStream, item: TokenStream) -> TokenStream {
    let args = match parse_cacheable_args(attr) {
        Ok(args) => args,
        Err(err) => return err.to_compile_error().into(),
    };
    let input = parse_macro_input!(item as ItemFn);
    match expand_cacheable(args, input) {
        Ok(expanded) => expanded.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

/// Write-through wrapper: executes function and updates cache on success.
#[proc_macro_attribute]
pub fn cache_put(attr: TokenStream, item: TokenStream) -> TokenStream {
    let args = match parse_cache_put_args(attr) {
        Ok(args) => args,
        Err(err) => return err.to_compile_error().into(),
    };
    let input = parse_macro_input!(item as ItemFn);
    match expand_cache_put(args, input) {
        Ok(expanded) => expanded.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

/// Invalidation wrapper: deletes cache before/after function based on `before` option.
#[proc_macro_attribute]
pub fn cache_evict(attr: TokenStream, item: TokenStream) -> TokenStream {
    let args = match parse_cache_evict_args(attr) {
        Ok(args) => args,
        Err(err) => return err.to_compile_error().into(),
    };
    let input = parse_macro_input!(item as ItemFn);
    match expand_cache_evict(args, input) {
        Ok(expanded) => expanded.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

/// Batch cache-first wrapper: `mget` first, then loads misses and `mset` backfill.
#[proc_macro_attribute]
pub fn cacheable_batch(attr: TokenStream, item: TokenStream) -> TokenStream {
    let args = match parse_cacheable_batch_args(attr) {
        Ok(args) => args,
        Err(err) => return err.to_compile_error().into(),
    };
    let input = parse_macro_input!(item as ItemFn);
    match expand_cacheable_batch(args, input) {
        Ok(expanded) => expanded.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

/// Batch invalidation wrapper: `mdel` before/after function according to `before`.
#[proc_macro_attribute]
pub fn cache_evict_batch(attr: TokenStream, item: TokenStream) -> TokenStream {
    let args = match parse_cache_evict_batch_args(attr) {
        Ok(args) => args,
        Err(err) => return err.to_compile_error().into(),
    };
    let input = parse_macro_input!(item as ItemFn);
    match expand_cache_evict_batch(args, input) {
        Ok(expanded) => expanded.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

fn expand_cacheable(
    args: CacheableArgs,
    mut item: ItemFn,
) -> syn::Result<proc_macro2::TokenStream> {
    ensure_async_method(&item.sig, "cacheable")?;
    let ok_ty = parse_ok_type(&item.sig, "cacheable")?;
    ensure_option_type(
        &ok_ty,
        "cacheable requires a result-like return type where Ok is `Option<T>`",
    )?;

    let runtime = runtime_crate_path();
    let on_get_error = on_cache_error_tokens(args.on_cache_error, &runtime, "cacheable", "get");
    let on_set_error = on_cache_error_tokens(args.on_cache_error, &runtime, "cacheable", "set");
    let cache_expr = args.cache;
    let key_expr = args.key;
    let allow_stale = args.allow_stale;
    let cache_none = args.cache_none;
    let original_block = item.block;

    item.block = Box::new(syn::parse_quote!({
        let __key = #key_expr;
        let __opts = #runtime::ReadOptions {
            allow_stale: #allow_stale,
            disable_load: true,
        };

        match (#cache_expr).get(&__key, &__opts).await {
            Ok(Some(__hit)) => return Ok(Some(__hit)),
            Ok(None) => {}
            Err(__cache_err) => {
                #on_get_error
            }
        }

        let __result = (async #original_block).await;
        match __result {
            Ok(__value) => {
                if let Some(__present) = __value.as_ref() {
                    if let Err(__cache_err) = (#cache_expr).set(&__key, Some(__present.clone())).await {
                        #on_set_error
                    }
                } else if #cache_none {
                    if let Err(__cache_err) = (#cache_expr).set(&__key, None).await {
                        #on_set_error
                    }
                }
                Ok(__value)
            }
            Err(__err) => Err(__err),
        }
    }));

    Ok(quote!(#item))
}

fn expand_cache_put(args: CachePutArgs, mut item: ItemFn) -> syn::Result<proc_macro2::TokenStream> {
    ensure_async_method(&item.sig, "cache_put")?;
    parse_ok_type(&item.sig, "cache_put")?;

    let runtime = runtime_crate_path();
    let on_set_error = on_cache_error_tokens(args.on_cache_error, &runtime, "cache_put", "set");
    let cache_expr = args.cache;
    let key_expr = args.key;
    let value_expr = args.value;
    let original_block = item.block;

    item.block = Box::new(syn::parse_quote!({
        let __key = #key_expr;

        let __result = (async #original_block).await;
        match __result {
            Ok(__ok) => {
                let __value = #value_expr;
                if let Err(__cache_err) = (#cache_expr).set(&__key, __value).await {
                    #on_set_error
                }
                Ok(__ok)
            }
            Err(__err) => Err(__err),
        }
    }));

    Ok(quote!(#item))
}

fn expand_cache_evict(
    args: CacheEvictArgs,
    mut item: ItemFn,
) -> syn::Result<proc_macro2::TokenStream> {
    ensure_async_method(&item.sig, "cache_evict")?;
    parse_ok_type(&item.sig, "cache_evict")?;

    let runtime = runtime_crate_path();
    let before = args.before;
    let on_before_error =
        on_cache_error_tokens(args.on_cache_error, &runtime, "cache_evict", "del_before");
    let on_after_error =
        on_cache_error_tokens(args.on_cache_error, &runtime, "cache_evict", "del_after");
    let cache_expr = args.cache;
    let key_expr = args.key;
    let original_block = item.block;

    item.block = Box::new(syn::parse_quote!({
        let __key = #key_expr;

        if #before {
            if let Err(__cache_err) = (#cache_expr).del(&__key).await {
                #on_before_error
            }
        }

        let __result = (async #original_block).await;
        match __result {
            Ok(__ok) => {
                if !#before {
                    if let Err(__cache_err) = (#cache_expr).del(&__key).await {
                        #on_after_error
                    }
                }
                Ok(__ok)
            }
            Err(__err) => Err(__err),
        }
    }));

    Ok(quote!(#item))
}

fn expand_cacheable_batch(
    args: CacheableBatchArgs,
    mut item: ItemFn,
) -> syn::Result<proc_macro2::TokenStream> {
    ensure_async_method(&item.sig, "cacheable_batch")?;
    let ok_ty = parse_ok_type(&item.sig, "cacheable_batch")?;
    ensure_hashmap_option_type(
        &ok_ty,
        "cacheable_batch requires a result-like return type where Ok is `HashMap<K, Option<V>>`",
    )?;

    let runtime = runtime_crate_path();
    let on_mget_error = on_cache_error_with_fallback_tokens(
        args.on_cache_error,
        &runtime,
        "cacheable_batch",
        "mget",
        quote!(::std::collections::HashMap::new()),
    );
    let on_mset_error =
        on_cache_error_tokens(args.on_cache_error, &runtime, "cacheable_batch", "mset");
    let cache_expr = args.cache;
    let keys_expr = args.keys;
    let allow_stale = args.allow_stale;
    let miss_keys_rebind = build_miss_keys_rebind(&item.sig, &keys_expr, "cacheable_batch")?;
    let original_block = item.block;

    item.block = Box::new(syn::parse_quote!({
        let __requested = {
            let mut keys = ::std::vec::Vec::new();
            let mut seen = ::std::collections::HashSet::new();
            for key in (#keys_expr).iter().cloned() {
                if seen.insert(key.clone()) {
                    keys.push(key);
                }
            }
            keys
        };

        if __requested.is_empty() {
            return Ok(::std::collections::HashMap::new());
        }

        let __opts = #runtime::ReadOptions {
            allow_stale: #allow_stale,
            disable_load: true,
        };

        let mut __values = match (#cache_expr).mget(&__requested, &__opts).await {
            Ok(cached) => cached,
            Err(__cache_err) => {
                #on_mget_error
            }
        };

        let __misses = __requested
            .iter()
            .filter(|key| match __values.get(*key) {
                Some(value) => value.is_none(),
                None => true,
            })
            .cloned()
            .collect::<::std::vec::Vec<_>>();

        if __misses.is_empty() {
            return Ok(__values);
        }

        #miss_keys_rebind

        let __result = (async #original_block).await;
        match __result {
            Ok(__loaded_map) => {
                let mut __writes = ::std::collections::HashMap::with_capacity(__misses.len());
                for key in __misses {
                    let loaded = __loaded_map.get(&key).cloned().unwrap_or(None);
                    __values.insert(key.clone(), loaded.clone());
                    __writes.insert(key, loaded);
                }

                if !__writes.is_empty() {
                    if let Err(__cache_err) = (#cache_expr).mset(__writes).await {
                        #on_mset_error
                    }
                }

                Ok(__values)
            }
            Err(__err) => Err(__err),
        }
    }));

    Ok(quote!(#item))
}

fn expand_cache_evict_batch(
    args: CacheEvictBatchArgs,
    mut item: ItemFn,
) -> syn::Result<proc_macro2::TokenStream> {
    ensure_async_method(&item.sig, "cache_evict_batch")?;
    parse_ok_type(&item.sig, "cache_evict_batch")?;

    let runtime = runtime_crate_path();
    let before = args.before;
    let on_before_error = on_cache_error_tokens(
        args.on_cache_error,
        &runtime,
        "cache_evict_batch",
        "mdel_before",
    );
    let on_after_error = on_cache_error_tokens(
        args.on_cache_error,
        &runtime,
        "cache_evict_batch",
        "mdel_after",
    );
    let cache_expr = args.cache;
    let keys_expr = args.keys;
    let original_block = item.block;

    item.block = Box::new(syn::parse_quote!({
        let __keys = {
            let mut keys = ::std::vec::Vec::new();
            let mut seen = ::std::collections::HashSet::new();
            for key in (#keys_expr).iter().cloned() {
                if seen.insert(key.clone()) {
                    keys.push(key);
                }
            }
            keys
        };

        if #before && !__keys.is_empty() {
            if let Err(__cache_err) = (#cache_expr).mdel(&__keys).await {
                #on_before_error
            }
        }

        let __result = (async #original_block).await;
        match __result {
            Ok(__ok) => {
                if !#before && !__keys.is_empty() {
                    if let Err(__cache_err) = (#cache_expr).mdel(&__keys).await {
                        #on_after_error
                    }
                }
                Ok(__ok)
            }
            Err(__err) => Err(__err),
        }
    }));

    Ok(quote!(#item))
}

/// Ensures the macro is used on an async method with a receiver.
fn ensure_async_method(sig: &Signature, macro_name: &str) -> syn::Result<()> {
    if sig.asyncness.is_none() {
        return Err(Error::new(
            sig.span(),
            format!(
                "{macro_name} only supports async methods, expected `async fn ...(&self, ...)`"
            ),
        ));
    }

    let Some(first) = sig.inputs.first() else {
        return Err(Error::new(
            sig.span(),
            format!(
                "{macro_name} requires a method receiver, expected first argument `&self` or `&mut self`"
            ),
        ));
    };

    if !matches!(first, FnArg::Receiver(_)) {
        return Err(Error::new(
            first.span(),
            format!(
                "{macro_name} only supports methods on impl blocks, expected first argument `&self` or `&mut self`"
            ),
        ));
    }

    Ok(())
}

/// Parses and validates result-like return type, then extracts the first generic type argument.
fn parse_ok_type(sig: &Signature, macro_name: &str) -> syn::Result<Type> {
    let ReturnType::Type(_, ty) = &sig.output else {
        return Err(Error::new(
            sig.span(),
            format!("{macro_name} requires a result-like return type"),
        ));
    };

    let Type::Path(TypePath { path, .. }) = ty.as_ref() else {
        return Err(Error::new(
            ty.span(),
            format!("{macro_name} requires a result-like return type"),
        ));
    };

    let Some(last) = path.segments.last() else {
        return Err(Error::new(
            path.span(),
            format!("{macro_name} requires a result-like return type"),
        ));
    };

    let PathArguments::AngleBracketed(args) = &last.arguments else {
        return Err(Error::new(
            last.arguments.span(),
            format!("{macro_name} requires a result-like return type"),
        ));
    };

    let mut type_args = args.args.iter().filter_map(|arg| {
        if let GenericArgument::Type(ty) = arg {
            Some(ty.clone())
        } else {
            None
        }
    });

    let Some(ok) = type_args.next() else {
        return Err(Error::new(
            args.span(),
            format!("{macro_name} requires a result-like return type"),
        ));
    };

    Ok(ok)
}

/// Ensures a type is `Option<T>`.
fn ensure_option_type(ty: &Type, message: &str) -> syn::Result<()> {
    let Type::Path(TypePath { path, .. }) = ty else {
        return Err(Error::new(ty.span(), message));
    };

    let Some(last) = path.segments.last() else {
        return Err(Error::new(path.span(), message));
    };

    if last.ident != "Option" {
        return Err(Error::new(last.ident.span(), message));
    }

    Ok(())
}

/// Ensures a type is `HashMap<K, Option<V>>`.
fn ensure_hashmap_option_type(ty: &Type, message: &str) -> syn::Result<()> {
    let Type::Path(TypePath { path, .. }) = ty else {
        return Err(Error::new(ty.span(), message));
    };

    let Some(last) = path.segments.last() else {
        return Err(Error::new(path.span(), message));
    };

    if last.ident != "HashMap" {
        return Err(Error::new(last.ident.span(), message));
    }

    let PathArguments::AngleBracketed(args) = &last.arguments else {
        return Err(Error::new(last.arguments.span(), message));
    };

    let mut type_args = args.args.iter().filter_map(|arg| {
        if let GenericArgument::Type(ty) = arg {
            Some(ty)
        } else {
            None
        }
    });

    let _key_ty = type_args.next();
    let Some(value_ty) = type_args.next() else {
        return Err(Error::new(args.span(), message));
    };

    ensure_option_type(value_ty, message)
}

/// Rebinds `keys` method parameter to cache misses when possible.
fn build_miss_keys_rebind(
    sig: &Signature,
    keys_expr: &Expr,
    macro_name: &str,
) -> syn::Result<proc_macro2::TokenStream> {
    let Expr::Path(path) = keys_expr else {
        return Ok(quote! {});
    };

    let Some(keys_ident) = path.path.get_ident() else {
        return Ok(quote! {});
    };

    let Some(keys_ty) = method_arg_type(sig, keys_ident) else {
        return Ok(quote! {});
    };

    match keys_ty {
        Type::Path(TypePath { path, .. }) => {
            let Some(last) = path.segments.last() else {
                return Ok(quote! {});
            };

            if last.ident == "Vec" {
                return Ok(quote! {
                    let #keys_ident = __misses.clone();
                });
            }
        }
        Type::Reference(reference) => match reference.elem.as_ref() {
            Type::Slice(_) => {
                return Ok(quote! {
                    let #keys_ident = __misses.as_slice();
                });
            }
            Type::Path(TypePath { path, .. }) => {
                let Some(last) = path.segments.last() else {
                    return Ok(quote! {});
                };

                if last.ident == "Vec" {
                    return Ok(quote! {
                        let #keys_ident = &__misses;
                    });
                }
            }
            _ => {}
        },
        _ => {}
    }

    Err(Error::new(
        keys_expr.span(),
        format!(
            "{macro_name} only supports `keys` parameter types `Vec<K>`, `&[K]` or `&Vec<K>` when reusing misses in function body"
        ),
    ))
}

/// Finds method argument type by identifier name.
fn method_arg_type(sig: &Signature, ident: &syn::Ident) -> Option<Type> {
    for arg in &sig.inputs {
        let FnArg::Typed(typed) = arg else {
            continue;
        };

        let syn::Pat::Ident(pat_ident) = typed.pat.as_ref() else {
            continue;
        };

        if pat_ident.ident == *ident {
            return Some((*typed.ty).clone());
        }
    }

    None
}

/// Generates error handling branch for cache operation failures.
fn on_cache_error_tokens(
    mode: OnCacheError,
    runtime: &proc_macro2::TokenStream,
    macro_name: &str,
    op_name: &str,
) -> proc_macro2::TokenStream {
    match mode {
        OnCacheError::Ignore => quote! {
            #runtime::tracing::warn!(
                target: "accelerator::macros",
                cache_macro = #macro_name,
                op = #op_name,
                result = "ignored",
                error = %__cache_err,
                "cache operation failed, continue business flow"
            );
        },
        OnCacheError::Propagate => quote! {
            #runtime::tracing::warn!(
                target: "accelerator::macros",
                cache_macro = #macro_name,
                op = #op_name,
                result = "propagated",
                error = %__cache_err,
                "cache operation failed, return error"
            );
            return Err(::core::convert::Into::into(__cache_err));
        },
    }
}

/// Generates error handling branch for cache operation failures with expression fallback.
fn on_cache_error_with_fallback_tokens(
    mode: OnCacheError,
    runtime: &proc_macro2::TokenStream,
    macro_name: &str,
    op_name: &str,
    fallback: proc_macro2::TokenStream,
) -> proc_macro2::TokenStream {
    match mode {
        OnCacheError::Ignore => quote! {
            #runtime::tracing::warn!(
                target: "accelerator::macros",
                cache_macro = #macro_name,
                op = #op_name,
                result = "ignored",
                error = %__cache_err,
                "cache operation failed, continue business flow"
            );
            #fallback
        },
        OnCacheError::Propagate => quote! {
            #runtime::tracing::warn!(
                target: "accelerator::macros",
                cache_macro = #macro_name,
                op = #op_name,
                result = "propagated",
                error = %__cache_err,
                "cache operation failed, return error"
            );
            return Err(::core::convert::Into::into(__cache_err));
        },
    }
}

/// Parses `cacheable` attribute arguments.
fn parse_cacheable_args(attr: TokenStream) -> syn::Result<CacheableArgs> {
    let metas = parse_attr_metas(attr)?;

    let mut cache = None;
    let mut key = None;
    let mut allow_stale = None;
    let mut cache_none = None;
    let mut on_cache_error = None;

    for meta in metas {
        let (name, value, span) = parse_name_value(meta)?;
        match name.as_str() {
            "cache" => set_once(&mut cache, value, "cache", span)?,
            "key" => set_once(&mut key, value, "key", span)?,
            "allow_stale" => set_once(
                &mut allow_stale,
                parse_bool_expr(&value, span)?,
                "allow_stale",
                span,
            )?,
            "cache_none" => set_once(
                &mut cache_none,
                parse_bool_expr(&value, span)?,
                "cache_none",
                span,
            )?,
            "on_cache_error" => set_once(
                &mut on_cache_error,
                parse_on_cache_error(&value, span)?,
                "on_cache_error",
                span,
            )?,
            _ => {
                return Err(Error::new(
                    span,
                    "unknown cacheable argument, supported: cache, key, allow_stale, cache_none, on_cache_error",
                ));
            }
        }
    }

    Ok(CacheableArgs {
        cache: required(cache, "cache")?,
        key: required(key, "key")?,
        allow_stale: allow_stale.unwrap_or(false),
        cache_none: cache_none.unwrap_or(true),
        on_cache_error: on_cache_error.unwrap_or(OnCacheError::Ignore),
    })
}

/// Parses `cache_put` attribute arguments.
fn parse_cache_put_args(attr: TokenStream) -> syn::Result<CachePutArgs> {
    let metas = parse_attr_metas(attr)?;

    let mut cache = None;
    let mut key = None;
    let mut value = None;
    let mut on_cache_error = None;

    for meta in metas {
        let (name, expr, span) = parse_name_value(meta)?;
        match name.as_str() {
            "cache" => set_once(&mut cache, expr, "cache", span)?,
            "key" => set_once(&mut key, expr, "key", span)?,
            "value" => set_once(&mut value, expr, "value", span)?,
            "on_cache_error" => set_once(
                &mut on_cache_error,
                parse_on_cache_error(&expr, span)?,
                "on_cache_error",
                span,
            )?,
            _ => {
                return Err(Error::new(
                    span,
                    "unknown cache_put argument, supported: cache, key, value, on_cache_error",
                ));
            }
        }
    }

    Ok(CachePutArgs {
        cache: required(cache, "cache")?,
        key: required(key, "key")?,
        value: required(value, "value")?,
        on_cache_error: on_cache_error.unwrap_or(OnCacheError::Ignore),
    })
}

/// Parses `cache_evict` attribute arguments.
fn parse_cache_evict_args(attr: TokenStream) -> syn::Result<CacheEvictArgs> {
    let metas = parse_attr_metas(attr)?;

    let mut cache = None;
    let mut key = None;
    let mut before = None;
    let mut on_cache_error = None;

    for meta in metas {
        let (name, value, span) = parse_name_value(meta)?;
        match name.as_str() {
            "cache" => set_once(&mut cache, value, "cache", span)?,
            "key" => set_once(&mut key, value, "key", span)?,
            "before" => set_once(&mut before, parse_bool_expr(&value, span)?, "before", span)?,
            "on_cache_error" => set_once(
                &mut on_cache_error,
                parse_on_cache_error(&value, span)?,
                "on_cache_error",
                span,
            )?,
            _ => {
                return Err(Error::new(
                    span,
                    "unknown cache_evict argument, supported: cache, key, before, on_cache_error",
                ));
            }
        }
    }

    Ok(CacheEvictArgs {
        cache: required(cache, "cache")?,
        key: required(key, "key")?,
        before: before.unwrap_or(false),
        on_cache_error: on_cache_error.unwrap_or(OnCacheError::Ignore),
    })
}

/// Parses `cacheable_batch` attribute arguments.
fn parse_cacheable_batch_args(attr: TokenStream) -> syn::Result<CacheableBatchArgs> {
    let metas = parse_attr_metas(attr)?;

    let mut cache = None;
    let mut keys = None;
    let mut allow_stale = None;
    let mut on_cache_error = None;

    for meta in metas {
        let (name, value, span) = parse_name_value(meta)?;
        match name.as_str() {
            "cache" => set_once(&mut cache, value, "cache", span)?,
            "keys" => set_once(&mut keys, value, "keys", span)?,
            "allow_stale" => set_once(
                &mut allow_stale,
                parse_bool_expr(&value, span)?,
                "allow_stale",
                span,
            )?,
            "on_cache_error" => set_once(
                &mut on_cache_error,
                parse_on_cache_error(&value, span)?,
                "on_cache_error",
                span,
            )?,
            _ => {
                return Err(Error::new(
                    span,
                    "unknown cacheable_batch argument, supported: cache, keys, allow_stale, on_cache_error",
                ));
            }
        }
    }

    Ok(CacheableBatchArgs {
        cache: required(cache, "cache")?,
        keys: required(keys, "keys")?,
        allow_stale: allow_stale.unwrap_or(false),
        on_cache_error: on_cache_error.unwrap_or(OnCacheError::Ignore),
    })
}

/// Parses `cache_evict_batch` attribute arguments.
fn parse_cache_evict_batch_args(attr: TokenStream) -> syn::Result<CacheEvictBatchArgs> {
    let metas = parse_attr_metas(attr)?;

    let mut cache = None;
    let mut keys = None;
    let mut before = None;
    let mut on_cache_error = None;

    for meta in metas {
        let (name, value, span) = parse_name_value(meta)?;
        match name.as_str() {
            "cache" => set_once(&mut cache, value, "cache", span)?,
            "keys" => set_once(&mut keys, value, "keys", span)?,
            "before" => set_once(&mut before, parse_bool_expr(&value, span)?, "before", span)?,
            "on_cache_error" => set_once(
                &mut on_cache_error,
                parse_on_cache_error(&value, span)?,
                "on_cache_error",
                span,
            )?,
            _ => {
                return Err(Error::new(
                    span,
                    "unknown cache_evict_batch argument, supported: cache, keys, before, on_cache_error",
                ));
            }
        }
    }

    Ok(CacheEvictBatchArgs {
        cache: required(cache, "cache")?,
        keys: required(keys, "keys")?,
        before: before.unwrap_or(false),
        on_cache_error: on_cache_error.unwrap_or(OnCacheError::Ignore),
    })
}

/// Parses comma-separated `name = value` metadata list.
fn parse_attr_metas(attr: TokenStream) -> syn::Result<Vec<Meta>> {
    let parser = Punctuated::<Meta, Token![,]>::parse_terminated;
    let metas = parser.parse(attr)?;
    Ok(metas.into_iter().collect())
}

/// Parses one `name = value` pair.
fn parse_name_value(meta: Meta) -> syn::Result<(String, Expr, proc_macro2::Span)> {
    let span = meta.span();
    let Meta::NameValue(MetaNameValue { path, value, .. }) = meta else {
        return Err(Error::new(span, "expected `name = value`"));
    };

    let Some(ident) = path.get_ident() else {
        return Err(Error::new(path.span(), "expected simple identifier"));
    };

    Ok((ident.to_string(), value, ident.span()))
}

/// Parses a boolean literal argument.
fn parse_bool_expr(expr: &Expr, span: proc_macro2::Span) -> syn::Result<bool> {
    let Expr::Lit(ExprLit {
        lit: Lit::Bool(value),
        ..
    }) = expr
    else {
        return Err(Error::new(span, "expected boolean literal"));
    };
    Ok(value.value())
}

/// Parses `on_cache_error` enum value.
fn parse_on_cache_error(expr: &Expr, span: proc_macro2::Span) -> syn::Result<OnCacheError> {
    let Expr::Lit(ExprLit {
        lit: Lit::Str(value),
        ..
    }) = expr
    else {
        return Err(Error::new(span, "expected string literal"));
    };

    match value.value().as_str() {
        "ignore" => Ok(OnCacheError::Ignore),
        "propagate" => Ok(OnCacheError::Propagate),
        _ => Err(Error::new(
            span,
            "on_cache_error must be \"ignore\" or \"propagate\"",
        )),
    }
}

/// Ensures a field is only assigned once in attribute arguments.
fn set_once<T>(
    slot: &mut Option<T>,
    value: T,
    name: &str,
    span: proc_macro2::Span,
) -> syn::Result<()> {
    if slot.is_some() {
        return Err(Error::new(span, format!("duplicate `{name}` argument")));
    }
    *slot = Some(value);
    Ok(())
}

/// Extracts required argument value.
fn required<T>(slot: Option<T>, name: &str) -> syn::Result<T> {
    slot.ok_or_else(|| Error::new(proc_macro2::Span::call_site(), format!("missing `{name}`")))
}

/// Resolves runtime crate path so macros work both inside and outside `accelerator`.
fn runtime_crate_path() -> proc_macro2::TokenStream {
    match crate_name("accelerator") {
        Ok(FoundCrate::Itself) => quote!(::accelerator),
        Ok(FoundCrate::Name(name)) => {
            let ident = format_ident!("{}", name);
            quote!(::#ident)
        }
        Err(_) => quote!(::accelerator),
    }
}
