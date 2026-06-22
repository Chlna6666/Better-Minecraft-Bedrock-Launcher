use proc_macro::TokenStream;
use quote::quote;
use syn::{Fields, Ident, ImplItem, ItemImpl, ItemStruct, parse_macro_input, parse_quote};

#[proc_macro_attribute]
pub fn hook_element(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let mut input = parse_macro_input!(item as ItemStruct);
    let struct_ident = input.ident.clone();
    let generics = input.generics.clone();
    let (impl_generics, type_generics, where_clause) = generics.split_for_impl();

    let constructor = match &mut input.fields {
        Fields::Named(fields) => {
            let original_fields = fields
                .named
                .iter()
                .map(|field| {
                    let field_ident = field
                        .ident
                        .as_ref()
                        .expect("named field should have an identifier")
                        .clone();
                    let field_ty = field.ty.clone();
                    (field_ident, field_ty)
                })
                .collect::<Vec<(Ident, syn::Type)>>();

            fields.named.push(parse_quote! {
                #[doc(hidden)]
                __gpui_hooks: ::std::cell::RefCell<::std::vec::Vec<Box<dyn ::std::any::Any>>>
            });
            fields.named.push(parse_quote! {
                #[doc(hidden)]
                __gpui_hook_index: ::std::cell::Cell<usize>
            });
            fields.named.push(parse_quote! {
                #[doc(hidden)]
                __gpui_hook_count: ::std::cell::Cell<usize>
            });

            if original_fields.is_empty() {
                quote! {
                    impl #impl_generics #struct_ident #type_generics #where_clause {
                        #[must_use]
                        pub fn new_hooked() -> Self {
                            Self {
                                __gpui_hooks: ::std::cell::RefCell::new(::std::vec::Vec::new()),
                                __gpui_hook_index: ::std::cell::Cell::new(0),
                                __gpui_hook_count: ::std::cell::Cell::new(0),
                            }
                        }
                    }

                    impl #impl_generics ::std::default::Default for #struct_ident #type_generics #where_clause {
                        fn default() -> Self {
                            Self::new_hooked()
                        }
                    }
                }
            } else {
                let params = original_fields
                    .iter()
                    .map(|(field_ident, field_ty)| quote! { #field_ident: #field_ty });
                let inits = original_fields
                    .iter()
                    .map(|(field_ident, _)| quote! { #field_ident });

                quote! {
                    impl #impl_generics #struct_ident #type_generics #where_clause {
                        #[must_use]
                        pub fn new_hooked(#(#params),*) -> Self {
                            Self {
                                #(#inits),*,
                                __gpui_hooks: ::std::cell::RefCell::new(::std::vec::Vec::new()),
                                __gpui_hook_index: ::std::cell::Cell::new(0),
                                __gpui_hook_count: ::std::cell::Cell::new(0),
                            }
                        }
                    }
                }
            }
        }
        Fields::Unit => {
            let named_fields: syn::FieldsNamed = parse_quote!({
                #[doc(hidden)]
                __gpui_hooks: ::std::cell::RefCell<::std::vec::Vec<Box<dyn ::std::any::Any>>>,
                #[doc(hidden)]
                __gpui_hook_index: ::std::cell::Cell<usize>,
                #[doc(hidden)]
                __gpui_hook_count: ::std::cell::Cell<usize>
            });
            input.fields = Fields::Named(named_fields);

            quote! {
                impl #impl_generics #struct_ident #type_generics #where_clause {
                    #[must_use]
                    pub fn new_hooked() -> Self {
                        Self {
                            __gpui_hooks: ::std::cell::RefCell::new(::std::vec::Vec::new()),
                            __gpui_hook_index: ::std::cell::Cell::new(0),
                            __gpui_hook_count: ::std::cell::Cell::new(0),
                        }
                    }
                }

                impl #impl_generics ::std::default::Default for #struct_ident #type_generics #where_clause {
                    fn default() -> Self {
                        Self::new_hooked()
                    }
                }
            }
        }
        Fields::Unnamed(_) => {
            return syn::Error::new_spanned(
                input,
                "#[hook_element] currently supports named-field structs and unit structs only",
            )
            .to_compile_error()
            .into();
        }
    };

    let expanded = quote! {
        #input

        #constructor

        impl #impl_generics ::gpui_hooks::hooks::HasHooks for #struct_ident #type_generics #where_clause {
            fn _hooks_storage(
                &self,
            ) -> &::std::cell::RefCell<::std::vec::Vec<Box<dyn ::std::any::Any>>> {
                &self.__gpui_hooks
            }

            fn _hook_index_cell(&self) -> &::std::cell::Cell<usize> {
                &self.__gpui_hook_index
            }

            fn _hook_count_cell(&self) -> &::std::cell::Cell<usize> {
                &self.__gpui_hook_count
            }
        }
    };

    expanded.into()
}

#[proc_macro_attribute]
pub fn hook_render(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let mut input = parse_macro_input!(item as ItemImpl);

    let Some((_, path, _)) = &input.trait_ else {
        return syn::Error::new_spanned(
            &input.self_ty,
            "#[hook_render] must be applied to an impl of gpui::Render",
        )
        .to_compile_error()
        .into();
    };

    let is_render_impl = path
        .segments
        .last()
        .is_some_and(|segment| segment.ident == "Render");

    if !is_render_impl {
        return syn::Error::new_spanned(
            path,
            "#[hook_render] must be applied to an impl of gpui::Render",
        )
        .to_compile_error()
        .into();
    }

    let Some(render_method) = input.items.iter_mut().find_map(|item| match item {
        ImplItem::Fn(method) if method.sig.ident == "render" => Some(method),
        _ => None,
    }) else {
        return syn::Error::new_spanned(
            &input.self_ty,
            "#[hook_render] could not find Render::render",
        )
        .to_compile_error()
        .into();
    };

    let original_block = render_method.block.clone();
    render_method.block = parse_quote!({
        ::gpui_hooks::hooks::HasHooks::_begin_hooks(self);
        let __gpui_rendered = (|| #original_block)();
        ::gpui_hooks::hooks::HasHooks::_finish_hooks(self);
        __gpui_rendered
    });

    quote!(#input).into()
}
