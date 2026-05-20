#![doc = include_str!("../README.md")]

use proc_macro::TokenStream;
use quote::quote;
use syn::{DeriveInput, parse_macro_input};

/// Derives `Component` — the explicit opt-in that lets a type be
/// stored on entities and matched by `Query`.
///
/// Emits an empty `impl ::spark_ecs::Component for YourType {}`. Because
/// the `Component` trait is declared `Send + Sync + 'static`, that impl
/// only compiles when the type actually satisfies those bounds: a struct
/// holding an `Rc`, `RefCell`, or raw pointer is rejected right at the
/// derive site. That rejection is the compile-time thread-safety proof
/// the M4 parallel scheduler will rely on when it hands component
/// storages across worker threads.
///
/// # Examples
///
/// ```rust,ignore
/// // `ignore`: this crate can't depend on `spark-ecs` (that would be a
/// // cycle), so its doctests can't import the trait. Runnable examples
/// // live in `lib/ecs/README.md`.
/// use spark_ecs::Component;
///
/// #[derive(Component)]
/// struct Position {
///     x: f32,
///     y: f32,
/// }
/// ```
#[proc_macro_derive(Component)]
pub fn derive_component(input: TokenStream) -> TokenStream {
    impl_marker(input, &quote!(::spark_ecs::Component))
}

/// Derives `Resource` — the explicit opt-in that lets a type be stored
/// as a `World` singleton and fetched via `Res<T>` / `ResMut<T>`.
///
/// Emits an empty `impl ::spark_ecs::Resource for YourType {}`. Unlike
/// the `Component` derive above, the `Resource` trait carries only a
/// `'static` bound, so a non-`Send` resource (a GPU handle, an OS
/// resource) still derives cleanly. Thread-safety for resources is
/// enforced later, at the scheduler, not here — see the trait docs.
///
/// # Examples
///
/// ```rust,ignore
/// // `ignore` for the same cycle reason as `Component` above.
/// use spark_ecs::Resource;
///
/// #[derive(Resource)]
/// struct FrameCount(u64);
/// ```
#[proc_macro_derive(Resource)]
pub fn derive_resource(input: TokenStream) -> TokenStream {
    impl_marker(input, &quote!(::spark_ecs::Resource))
}

/// Emits an empty marker `impl #trait_path for #Type {}`, threading the
/// annotated type's own generics through unchanged.
///
/// `trait_path` arrives fully qualified (`::spark_ecs::Component`) so the
/// generated impl resolves the trait from the `spark_ecs` crate at the
/// *use* site — including inside `spark_ecs` itself, which makes that
/// path resolve with `extern crate self as spark_ecs;`.
///
/// [`split_for_impl`](syn::Generics::split_for_impl) returns the three
/// fragments an `impl` block needs — `impl<…>`, `Type<…>`, and the
/// `where` clause — so a generic `Wrapper<T>` yields
/// `impl<T> … for Wrapper<T> where …`. Generic parameters are *not*
/// auto-bounded `'static` here; concrete components (the only kind
/// today) are unaffected, and bound injection is a tracked follow-up.
fn impl_marker(input: TokenStream, trait_path: &proc_macro2::TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();
    quote! {
        impl #impl_generics #trait_path for #name #ty_generics #where_clause {}
    }
    .into()
}
