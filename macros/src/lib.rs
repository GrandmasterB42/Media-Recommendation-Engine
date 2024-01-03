extern crate proc_macro;
extern crate self as media_recommendation_engine;

use std::{ops::RangeFrom, path::PathBuf};

use quote::quote;
use syn::{parse_macro_input, Ident};

#[derive(Debug)]
struct TemplateInput {
    var: syn::Ident,
    engine: syn::Ident,
    path: String,
    ident: syn::Ident,
}

impl syn::parse::Parse for TemplateInput {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let var = input.parse()?;
        input.parse::<syn::Token![,]>()?;
        let engine = input.parse()?;
        input.parse::<syn::Token![,]>()?;
        let path = input.parse::<syn::LitStr>()?.value();
        input.parse::<syn::Token![,]>()?;
        let ident = input.parse()?;
        Ok(Self {
            var,
            engine,
            path,
            ident,
        })
    }
}

/// takes: basically like fn(&mut Template, TemplatingEngine, String, Ident)
/// Ident is the Identifier used for targeting the template
#[proc_macro]
pub fn template(items: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let input = parse_macro_input!(items as TemplateInput);
    let var = input.var;
    let engine = input.engine;
    let path = input.path;
    let targetident = input.ident;

    let mut current_environment = PathBuf::new();
    current_environment.push(env!("CARGO_MANIFEST_DIR"));
    current_environment.push(&path);

    let mut template: &str = &std::fs::read_to_string(current_environment)
        .expect("failed to read template during compilation");

    let mut targets = Vec::new();
    while let Some((_before, after)) = template.split_once('{') {
        if !after.starts_with('{') {
            template = after;
            continue;
        }
        let Some((inside, after)) = after.split_once('}') else {
            break;
        };
        if !after.starts_with('}') {
            template = after;
            continue;
        }

        let inside = &inside[1..];
        targets.push(inside);
        let after = &after[1..];
        template = after;
    }

    let targets: Vec<_> = targets
        .iter()
        .map(|x| syn::parse_str::<Ident>(x))
        .filter_map(|x| x.ok())
        .collect();

    let nums: RangeFrom<isize> = 0..;

    quote! {
        /// Automatically generated targets, these might now always be up to date, rebuild your proc macros!
        #[derive(Debug, Clone, Copy)]
        enum #targetident {
            #(#targets,)*
        }

        impl std::convert::Into<isize> for #targetident {
            fn into(self) -> isize {
                match self {
                    #(#targetident::#targets => #nums,)*
                }
            }
        }

        let mut #var = #engine.get::<#targetident>(#path).await;
    }
    .into()
}
