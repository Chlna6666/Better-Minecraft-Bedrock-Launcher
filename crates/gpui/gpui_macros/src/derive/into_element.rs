use proc_macro::TokenStream;
use quote::quote;
use syn::{DeriveInput, parse_macro_input};

pub fn derive_into_element(input: TokenStream) -> TokenStream {
    let ast = parse_macro_input!(input as DeriveInput);
    let type_name = &ast.ident;
    let (impl_generics, type_generics, where_clause) = ast.generics.split_for_impl();

    // BMCBL keeps RenderOnce components separate from rendered Entity<V> views.
    // Component<C> remains the consumed-component lifecycle wrapper, while
    // ViewElement is the non-generic erased lifecycle for Entity<V>/AnyView.
    let r#gen = quote! {
        impl #impl_generics gpui::IntoElement for #type_name #type_generics
        #where_clause
        {
            type Element = gpui::Component<Self>;

            #[track_caller]
            fn into_element(self) -> Self::Element {
                gpui::Component::new(self)
            }

            #[track_caller]
            #[inline(never)]
            fn into_any_element(self) -> gpui::AnyElement {
                gpui::Element::into_any(self.into_element())
            }
        }
    };

    r#gen.into()
}
