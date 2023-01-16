//! Module to parse entrypoint functions attributes with parameters

use syn::{
    parse::{Parse, ParseStream, Result},
    punctuated::Punctuated,
};

mod kw {
    syn::custom_keyword!(params);
}

/// Trait parameter type should implement to successfully construct arguments
pub trait ConstructArg {
    /// Construct argument expression based on the `self` value
    fn construct_arg(&self) -> syn::Expr;
}

/// Attribute with expected parameters for smart contract entrypoint function
pub struct ParamsAttr<P> {
    _params_kw: kw::params,
    _equal: syn::token::Eq,
    params: Params<P>,
}

impl<P: Parse> Parse for ParamsAttr<P> {
    fn parse(input: ParseStream) -> Result<Self> {
        let params_kw = input.parse()?;
        let equal = input.parse()?;
        let params_str: syn::LitStr = input.parse()?;
        let params = syn::parse_str(&params_str.value())?;
        Ok(ParamsAttr {
            _params_kw: params_kw,
            _equal: equal,
            params,
        })
    }
}

impl<P: ConstructArg> ParamsAttr<P> {
    /// Construct arguments for the entrypoint function
    pub fn construct_args(&self) -> Punctuated<syn::Expr, syn::token::Comma> {
        self.params
            .types
            .iter()
            .map(ConstructArg::construct_arg)
            .collect()
    }
}

/// Collection of parameter types that the smart contract entrypoint function is expecting
pub struct Params<P> {
    _bracket_token: syn::token::Bracket,
    types: Punctuated<P, syn::token::Comma>,
}

impl<P: Parse> Parse for Params<P> {
    fn parse(input: ParseStream) -> Result<Self> {
        let content;
        let bracket_token = syn::bracketed!(content in input);

        Ok(Params {
            _bracket_token: bracket_token,
            types: content.parse_terminated(P::parse)?,
        })
    }
}