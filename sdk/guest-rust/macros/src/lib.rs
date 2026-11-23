use proc_macro::TokenStream;
use quote::quote;
use syn::{ItemFn, parse_macro_input};

/// Export a synchronous `fn(&mut Context<H>, &[Event]) -> Result<(), HostError>`.
#[proc_macro_attribute]
pub fn coglet(attr: TokenStream, item: TokenStream) -> TokenStream {
    if !attr.is_empty() {
        return syn::Error::new(
            proc_macro::Span::call_site().into(),
            "coglet takes no arguments",
        )
        .to_compile_error()
        .into();
    }
    let function = parse_macro_input!(item as ItemFn);
    if function.sig.asyncness.is_some() || function.sig.unsafety.is_some() {
        return syn::Error::new_spanned(&function.sig, "coglet must be safe and synchronous")
            .to_compile_error()
            .into();
    }
    let name = &function.sig.ident;
    quote! {
        #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
        #function
        #[cfg(target_arch = "wasm32")]
        #[unsafe(no_mangle)]
        pub extern "C" fn abi_alloc(len: u32) -> u32 {
            ::coglet_guest_rust::__private::allocate(len)
        }
        #[cfg(target_arch = "wasm32")]
        #[unsafe(no_mangle)]
        pub extern "C" fn handle(ptr: u32, len: u32, step: u64, logical_time: u64, seed: u64) -> i32 {
            ::coglet_guest_rust::__private::invoke(ptr, len, step, logical_time, seed, #name)
        }
    }.into()
}
