//! This module provides parsing of custom attributes from the [`getset`](https://docs.rs/getset/latest/getset/) crate

use std::{collections::hash_map::Entry, fmt::Display, str::FromStr};

use proc_macro2::Span;
use rustc_hash::{FxHashMap, FxHashSet};
use syn2::{parse::ParseStream, punctuated::Punctuated, Attribute, Token};

use crate::attr_parse::derive::{Derive, DeriveAttr};

/// Type of accessor method derived for a structure
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GetSetDerive {
    Setters,
    Getters,
    MutGetters,
    CopyGetters,
}

impl GetSetDerive {
    pub fn try_from_path(path: &syn2::Path) -> Option<Self> {
        // try to be smart and handle two cases:
        // - bare attribute name (like `Getters`, when it's imported)
        // - fully qualified path (like `getset::Getters`, when it's not imported)
        let ident = if let Some(i) = path.get_ident() {
            i.clone()
        } else {
            let mut segments = path.segments.iter();
            if segments.len() == 2
                && segments.next().unwrap().ident.to_string().as_str() == "getset"
            {
                segments.next().unwrap().ident.clone()
            } else {
                return None;
            }
        };

        match ident.to_string().as_str() {
            "Setters" => Some(Self::Setters),
            "Getters" => Some(Self::Getters),
            "MutGetters" => Some(Self::MutGetters),
            "CopyGetters" => Some(Self::CopyGetters),
            _ => None,
        }
    }

    pub fn get_mode(self) -> GetSetGenMode {
        match self {
            Self::Setters => GetSetGenMode::Set,
            Self::Getters => GetSetGenMode::Get,
            Self::MutGetters => GetSetGenMode::GetMut,
            Self::CopyGetters => GetSetGenMode::GetCopy,
        }
    }
}

#[derive(Default, Debug, Eq, PartialEq, Clone)]
pub struct GetSetOptions {
    pub visibility: Option<syn2::Visibility>,
    pub with_prefix: bool,
}

struct SpannedGetSetOptions {
    span: Span,
    options: GetSetOptions,
}

impl syn2::parse::Parse for SpannedGetSetOptions {
    fn parse(input: ParseStream) -> syn2::Result<Self> {
        let mut result = GetSetOptions::default();
        // an accumulator for syn errors?
        // this is getting out of hand...
        // we need an accumulator to rule them all!
        let mut errors = Vec::new();

        let lit = input.parse::<syn2::LitStr>()?;
        for part in lit.value().split(' ') {
            if part == "with_prefix" {
                result.with_prefix = true;
            } else if let Ok(vis) = syn2::parse_str::<syn2::Visibility>(part) {
                if result.visibility.is_none() {
                    result.visibility = Some(vis);
                } else {
                    errors.push(syn2::Error::new(
                        lit.span(),
                        format!(
                            "Failed to parse getset options at {}: duplicate visibility",
                            part
                        ),
                    ));
                }
            } else {
                errors.push(syn2::Error::new(lit.span(), format!("Failed to parse getset options at `{}`: expected visibility or `with_prefix`", part)));
            }
        }

        if errors.is_empty() {
            Ok(SpannedGetSetOptions {
                span: lit.span(),
                options: result,
            })
        } else {
            let mut errors = errors.into_iter();
            let mut error = errors.next().expect("darling::Error can never be empty");

            for next_error in errors {
                error.combine(next_error);
            }

            Err(error)
        }
    }
}

#[derive(Debug, PartialEq, Eq, Hash, Copy, Clone)]
pub enum GetSetGenMode {
    Get,
    GetCopy,
    Set,
    GetMut,
}

impl Display for GetSetGenMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GetSetGenMode::Get => write!(f, "get"),
            GetSetGenMode::GetCopy => write!(f, "get_copy"),
            GetSetGenMode::Set => write!(f, "set"),
            GetSetGenMode::GetMut => write!(f, "get_mut"),
        }
    }
}

impl FromStr for GetSetGenMode {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "get" => Ok(GetSetGenMode::Get),
            "get_copy" => Ok(GetSetGenMode::GetCopy),
            "set" => Ok(GetSetGenMode::Set),
            "get_mut" => Ok(GetSetGenMode::GetMut),
            _ => Err(()),
        }
    }
}

enum GetSetAttrToken {
    Skip,
    Gen(GetSetGenMode, GetSetOptions),
}

struct SpannedGetSetAttrToken {
    span: Span,
    token: GetSetAttrToken,
}

impl syn2::parse::Parse for SpannedGetSetAttrToken {
    fn parse(input: ParseStream) -> syn2::Result<Self> {
        let ident = input.parse::<syn2::Ident>()?;

        match ident.to_string().as_str() {
            "skip" => Ok(SpannedGetSetAttrToken {
                span: ident.span(),
                token: GetSetAttrToken::Skip,
            }),
            s @ ("get" | "get_copy" | "set" | "get_mut") => {
                let mode = s.parse().unwrap();

                if input.peek(Token![=]) {
                    input.parse::<Token![=]>()?;
                    let options = input.parse::<SpannedGetSetOptions>()?;
                    let span = ident
                        .span()
                        .join(options.span)
                        .expect("must be in the same file");

                    Ok(SpannedGetSetAttrToken {
                        span,
                        token: GetSetAttrToken::Gen(mode, options.options),
                    })
                } else {
                    Ok(SpannedGetSetAttrToken {
                        span: ident.span(),
                        token: GetSetAttrToken::Gen(mode, GetSetOptions::default()),
                    })
                }
            }
            _ => Err(syn2::Error::new(
                ident.span(),
                "expected one of `get`, `get_copy`, `get_mut`, `set`, `skip`",
            )),
        }
    }
}

type RequestedAccessors = FxHashMap<GetSetGenMode, GetSetOptions>;

/// Insert an accessor into the map, emitting an error if such kind of accessor is already present in the map
fn insert_gen_request(
    accumulator: &mut darling::error::Accumulator,
    gen_map: &mut RequestedAccessors,
    span: Span,
    mode: GetSetGenMode,
    options: GetSetOptions,
) {
    if options.with_prefix && mode == GetSetGenMode::Set {
        accumulator.push(
            darling::Error::custom("`with_prefix` is not supported for `set`").with_span(&span),
        );
    }

    match gen_map.entry(mode) {
        Entry::Occupied(_) => accumulator.push(
            darling::Error::custom(format!("duplicate `getset({})` attribute", mode))
                .with_span(&span),
        ),
        Entry::Vacant(v) => {
            v.insert(options);
        }
    }
}

#[derive(Default, Debug, Eq, PartialEq, Clone)]
pub struct GetSetFieldAttr {
    pub skip: bool,
    pub gen: RequestedAccessors,
}

impl darling::FromAttributes for GetSetFieldAttr {
    fn from_attributes(attrs: &[Attribute]) -> darling::Result<Self> {
        let mut accumulator = darling::error::Accumulator::default();
        let mut skip_span = None;
        let mut result = GetSetFieldAttr {
            skip: false,
            gen: FxHashMap::default(),
        };
        for attr in attrs {
            // getset crate is quite liberal in what it accepts
            // it allows both the `#[getset(get)]` and `#[get]` syntax to be used
            // iroha doesn't use the latter form, so it is not supported by `iroha_ffi_derive`
            if attr.path().is_ident("getset") {
                let Some(list) = accumulator.handle(attr.meta.require_list().map_err(Into::into))
                    else { continue };
                let Some(tokens): Option<Punctuated<SpannedGetSetAttrToken, Token![,]>>
                    = accumulator.handle(list.parse_args_with(Punctuated::parse_terminated).map_err(Into::into))
                    else { continue };

                for token in tokens {
                    match token.token {
                        GetSetAttrToken::Skip => {
                            result.skip = true;
                            skip_span = Some(token.span);
                        }
                        GetSetAttrToken::Gen(mode, options) => insert_gen_request(
                            &mut accumulator,
                            &mut result.gen,
                            token.span,
                            mode,
                            options,
                        ),
                    }
                }
            } else if attr
                .path()
                .get_ident()
                .and_then(|ident| GetSetGenMode::from_str(&ident.to_string()).ok())
                .is_some()
            {
                accumulator.push(
                    darling::Error::custom(
                        "getset attributes without `getset` prefix are not supported by iroha_ffi_derive",
                    )
                    .with_span(attr),
                )
            }
        }

        if result.skip && !result.gen.is_empty() {
            accumulator.push(
                darling::Error::custom(
                    "`skip` is used, but attributes requesting a getter or setter are also present",
                )
                .with_span(&skip_span.unwrap()),
            );
        }

        accumulator.finish_with(result)
    }
}

#[derive(Default, Debug, Eq, PartialEq, Clone)]
pub struct GetSetStructAttr {
    pub gen: FxHashMap<GetSetGenMode, GetSetOptions>,
}

impl darling::FromAttributes for GetSetStructAttr {
    fn from_attributes(attrs: &[Attribute]) -> darling::Result<Self> {
        let mut accumulator = darling::error::Accumulator::default();
        let mut result = GetSetStructAttr {
            gen: FxHashMap::default(),
        };
        for attr in attrs {
            if attr.path().is_ident("getset") {
                let Some(list) = accumulator.handle(attr.meta.require_list().map_err(Into::into))
                    else { continue };
                let Some(tokens): Option<Punctuated<SpannedGetSetAttrToken, Token![,]>>
                    = accumulator.handle(list.parse_args_with(Punctuated::parse_terminated).map_err(Into::into))
                    else { continue };

                for token in tokens {
                    match token.token {
                        GetSetAttrToken::Skip => {
                            accumulator.push(
                                darling::Error::custom("`skip` is not valid on a struct")
                                    .with_span(&token.span),
                            );
                        }
                        GetSetAttrToken::Gen(mode, options) => insert_gen_request(
                            &mut accumulator,
                            &mut result.gen,
                            token.span,
                            mode,
                            options,
                        ),
                    }
                }
            } else if attr
                .path()
                .get_ident()
                .and_then(|ident| GetSetGenMode::from_str(&ident.to_string()).ok())
                .is_some()
            {
                accumulator.push(
                    darling::Error::custom(
                        "getset attributes without `getset` prefix are not supported by iroha_ffi_derive",
                    )
                        .with_span(attr),
                )
            }
        }

        accumulator.finish_with(result)
    }
}

impl GetSetFieldAttr {
    pub fn get_field_accessors(
        &self,
        derives: &DeriveAttr,
        struct_attr: &GetSetStructAttr,
    ) -> RequestedAccessors {
        if self.skip {
            return FxHashMap::default();
        }

        let mut result = struct_attr.gen.clone();
        for (mode, options) in &self.gen {
            match result.entry(*mode) {
                Entry::Occupied(mut o) => {
                    let o = o.get_mut();
                    // visibility is overwritten, while the "with_prefix" is merged
                    o.visibility = options.visibility.clone();
                    o.with_prefix |= options.with_prefix;
                }
                Entry::Vacant(v) => {
                    v.insert(options.clone());
                }
            }
        }

        // filter out the modes that are not requested by the `#[derive(...)]` attribute
        let derived_modes = derives
            .derives
            .iter()
            .filter_map(|d| match d {
                Derive::GetSet(derive) => Some(derive.get_mode()),
                _ => None,
            })
            .collect::<FxHashSet<_>>();
        result.retain(|&mode, _| derived_modes.contains(&mode));

        result
    }
}

#[cfg(test)]
mod test {
    use super::{
        GetSetFieldAttr, GetSetGenMode, GetSetOptions, GetSetStructAttr, RequestedAccessors,
    };

    mod parse {
        use darling::FromAttributes;
        use proc_macro2::TokenStream;
        use quote::quote;
        use rustc_hash::FxHashMap;
        use syn2::{parse::ParseStream, parse_quote, Attribute};

        use super::{GetSetFieldAttr, GetSetGenMode, GetSetOptions, GetSetStructAttr};

        // TODO: this can go into a common module
        fn parse_attributes(ts: TokenStream) -> Vec<Attribute> {
            struct Attributes(Vec<Attribute>);
            impl syn2::parse::Parse for Attributes {
                fn parse(input: ParseStream) -> syn2::Result<Self> {
                    Attribute::parse_outer(input).map(Attributes)
                }
            }

            syn2::parse2::<Attributes>(ts)
                .expect("Failed to parse attributes")
                .0
        }

        macro_rules! assert_getset_ok {
        ($( #[$meta:meta] )*,
            $ty:ident $body:tt
        ) => {
            {
                assert_eq!(
                    $ty::from_attributes(&parse_attributes(quote! {
                        $( #[$meta] )*
                    }))
                    .unwrap_or_else(|e| panic!("Parsing {} from attributes failed: {:#}", stringify!($ty), e)),
                    $ty $body
                );
            }
        };
    }

        #[test]
        fn field_empty() {
            assert_getset_ok!(
                #[abra_cadabra], // unrelated attr
                GetSetFieldAttr {
                    ..Default::default()
                }
            );
        }

        #[test]
        fn struct_empty() {
            assert_getset_ok!(
                #[abra_cadabra], // unrelated attr
                GetSetStructAttr {
                    ..Default::default()
                }
            );
        }

        #[test]
        fn field_skip() {
            assert_getset_ok!(
                #[getset(skip)],
                GetSetFieldAttr {
                    skip: true,
                    ..Default::default()
                }
            );
        }

        #[test]
        fn field_get() {
            assert_getset_ok!(
                #[getset(get)],
                GetSetFieldAttr {
                    gen: FxHashMap::from_iter([
                        (GetSetGenMode::Get, GetSetOptions::default()),
                    ]),
                    ..Default::default()
                }
            );
        }

        #[test]
        fn field_get_pub() {
            assert_getset_ok!(
                #[getset(get = "pub")],
                GetSetFieldAttr {
                    gen: FxHashMap::from_iter([
                        (GetSetGenMode::Get, GetSetOptions {
                            visibility: Some(parse_quote! { pub }),
                            ..Default::default()
                        }),
                    ]),
                    ..Default::default()
                }
            );
        }

        #[test]
        fn field_get_pub_with_prefix() {
            assert_getset_ok!(
                #[getset(get = "pub with_prefix")],
                GetSetFieldAttr {
                    gen: FxHashMap::from_iter([
                        (GetSetGenMode::Get, GetSetOptions {
                            visibility: Some(parse_quote! { pub }),
                            with_prefix: true,
                        }),
                    ]),
                    ..Default::default()
                }
            );
            assert_getset_ok!(
                #[getset(get = "with_prefix pub")],
                GetSetFieldAttr {
                    gen: FxHashMap::from_iter([
                        (GetSetGenMode::Get, GetSetOptions {
                            visibility: Some(parse_quote! { pub }),
                            with_prefix: true,
                        }),
                    ]),
                    ..Default::default()
                }
            );
        }

        #[test]
        fn struct_get() {
            assert_getset_ok!(
                #[getset(get)],
                GetSetStructAttr {
                    gen: FxHashMap::from_iter([
                        (GetSetGenMode::Get, GetSetOptions::default()),
                    ])
                }
            );
        }

        #[test]
        fn struct_get_pub() {
            assert_getset_ok!(
                #[getset(get = "pub")],
                GetSetStructAttr {
                    gen: FxHashMap::from_iter([
                        (GetSetGenMode::Get, GetSetOptions {
                            visibility: Some(parse_quote! { pub }),
                            ..Default::default()
                        }),
                    ])
                }
            );
        }

        #[test]
        fn struct_get_pub_with_prefix() {
            assert_getset_ok!(
                #[getset(get = "pub with_prefix")],
                GetSetStructAttr {
                    gen: FxHashMap::from_iter([
                        (GetSetGenMode::Get, GetSetOptions {
                            visibility: Some(parse_quote! { pub }),
                            with_prefix: true,
                        }),
                    ])
                }
            );
            assert_getset_ok!(
                #[getset(get = "with_prefix pub")],
                GetSetStructAttr {
                    gen: FxHashMap::from_iter([
                        (GetSetGenMode::Get, GetSetOptions {
                            visibility: Some(parse_quote! { pub }),
                            with_prefix: true,
                        }),
                    ])
                }
            );
        }

        #[test]
        fn field_get_copy() {
            assert_getset_ok!(
                #[getset(get_copy)],
                GetSetFieldAttr {
                    gen: FxHashMap::from_iter([
                        (GetSetGenMode::GetCopy, GetSetOptions::default()),
                    ]),
                    ..Default::default()
                }
            );
        }

        #[test]
        fn field_set() {
            assert_getset_ok!(
                #[getset(set)],
                GetSetFieldAttr {
                    gen: FxHashMap::from_iter([
                        (GetSetGenMode::Set, GetSetOptions::default()),
                    ]),
                    ..Default::default()
                }
            );
        }

        #[test]
        fn field_get_mut() {
            assert_getset_ok!(
                #[getset(get_mut)],
                GetSetFieldAttr {
                    gen: FxHashMap::from_iter([
                        (GetSetGenMode::GetMut, GetSetOptions::default()),
                    ]),
                    ..Default::default()
                }
            );
        }

        #[test]
        fn struct_get_copy() {
            assert_getset_ok!(
                #[getset(get_copy)],
                GetSetStructAttr {
                    gen: FxHashMap::from_iter([
                        (GetSetGenMode::GetCopy, GetSetOptions::default()),
                    ])
                }
            );
        }

        #[test]
        fn struct_set() {
            assert_getset_ok!(
                #[getset(set)],
                GetSetStructAttr {
                    gen: FxHashMap::from_iter([
                        (GetSetGenMode::Set, GetSetOptions::default()),
                    ])
                }
            );
        }

        #[test]
        fn struct_get_mut() {
            assert_getset_ok!(
                #[getset(get_mut)],
                GetSetStructAttr {
                    gen: FxHashMap::from_iter([
                        (GetSetGenMode::GetMut, GetSetOptions::default()),
                    ])
                }
            );
        }

        macro_rules! assert_getset_err {
        ($( #[$meta:meta] )*, $ty:ident, $error:expr) => {
            assert_eq!(
                $ty::from_attributes(&parse_attributes(quote! {
                    $( #[$meta] )*
                }))
                .unwrap_err()
                .to_string(),
                $error,
                "The error message does not match the expected one"
            )
        };
    }

        #[test]
        fn err_unknown_token() {
            assert_getset_err!(
                #[getset(unknown_token)],
                GetSetStructAttr,
                "expected one of `get`, `get_copy`, `get_mut`, `set`, `skip`"
            );
        }

        #[test]
        fn err_skip_struct() {
            assert_getset_err!(
                #[getset(skip)],
                GetSetStructAttr,
                "`skip` is not valid on a struct"
            );
        }

        #[test]
        fn err_duplicate_accessor() {
            assert_getset_err!(
                #[getset(get = "pub", get)],
                GetSetStructAttr,
                "duplicate `getset(get)` attribute"
            );
        }

        #[test]
        fn err_unknown_option() {
            assert_getset_err!(
                #[getset(get = "aboba")],
                GetSetStructAttr,
                "Failed to parse getset options at `aboba`: expected visibility or `with_prefix`"
            );
        }
    }
    mod inheritance {
        use darling::FromAttributes;
        use proc_macro2::TokenStream;
        use quote::quote;
        use syn2::{parse::ParseStream, parse_quote, Attribute};

        use super::{
            GetSetFieldAttr, GetSetGenMode, GetSetOptions, GetSetStructAttr, RequestedAccessors,
        };
        use crate::attr_parse::derive::DeriveAttr;

        fn get_field_derives(
            derive: TokenStream,
            struct_attr: TokenStream,
            field_attr: TokenStream,
        ) -> RequestedAccessors {
            fn parse_attributes<T: FromAttributes>(ts: TokenStream) -> T {
                struct Attributes(Vec<Attribute>);
                impl syn2::parse::Parse for Attributes {
                    fn parse(input: ParseStream) -> syn2::Result<Self> {
                        Attribute::parse_outer(input).map(Attributes)
                    }
                }

                let attrs = syn2::parse2::<Attributes>(ts)
                    .expect("Failed to parse attributes")
                    .0;
                T::from_attributes(&attrs).expect("Failed to parse attributes")
            }

            let derive = parse_attributes::<DeriveAttr>(derive);
            let struct_attr = parse_attributes::<GetSetStructAttr>(struct_attr);
            let field_attr = parse_attributes::<GetSetFieldAttr>(field_attr);

            field_attr.get_field_accessors(&derive, &struct_attr)
        }

        macro_rules! assert_getset_ok {
            (
                $( #[$derive:meta] )*,
                $( #[$struct_attr:meta] )*,
                $( #[$field_attr:meta] )*,
                $expected:expr
            ) => {
                assert_eq!(
                    get_field_derives(
                        quote! { $( #[$derive] )* },
                        quote! { $( #[$struct_attr] )* },
                        quote! { $( #[$field_attr] )* },
                    ),
                    $expected
                )
            };
        }

        #[test]
        fn getset_basic() {
            assert_getset_ok!(
                #[derive(Getters, Setters)],
                ,
                #[getset(get, set)],
                RequestedAccessors::from_iter([
                    (GetSetGenMode::Get, GetSetOptions::default()),
                    (GetSetGenMode::Set, GetSetOptions::default()),
                ])
            );
        }

        #[test]
        fn getset_derive_disabled() {
            // no Setters - no Set generated
            assert_getset_ok!(
                #[derive(Getters)],
                ,
                #[getset(get, set)],
                RequestedAccessors::from_iter([
                    (GetSetGenMode::Get, GetSetOptions::default())
                ])
            );
        }

        #[test]
        fn getset_inherit() {
            assert_getset_ok!(
                #[derive(Getters, Setters)],
                #[getset(get)],
                #[getset(set)],
                RequestedAccessors::from_iter([
                    (GetSetGenMode::Get, GetSetOptions::default()),
                    (GetSetGenMode::Set, GetSetOptions::default()),
                ])
            )
        }

        #[test]
        fn getset_overwrite_visibility() {
            assert_getset_ok!(
                #[derive(Getters, Setters)],
                #[getset(get = "pub(crate)", set = "pub(crate)")],
                #[getset(set = "pub")],
                RequestedAccessors::from_iter([
                    (GetSetGenMode::Get, GetSetOptions {
                        visibility: Some(parse_quote! { pub(crate) }),
                        ..Default::default()
                    }),
                    (GetSetGenMode::Set, GetSetOptions {
                        visibility: Some(parse_quote! { pub }),
                        ..Default::default()
                    }),
                ])
            )
        }

        #[test]
        fn inherit_with_prefix() {
            assert_getset_ok!(
                #[derive(Getters, CopyGetters)],
                #[getset(get = "with_prefix", get_copy = "pub")],
                #[getset(get = "pub", get_copy = "pub(crate) with_prefix")],
                RequestedAccessors::from_iter([
                    (GetSetGenMode::Get, GetSetOptions {
                        visibility: Some(parse_quote! { pub }),
                        with_prefix: true,
                    }),
                    (GetSetGenMode::GetCopy, GetSetOptions {
                        visibility: Some(parse_quote! { pub(crate) }),
                        with_prefix: true,
                    }),
                ])
            )
        }
    }
}