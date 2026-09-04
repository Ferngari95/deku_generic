//! Code generation.
//!
//! Each requested instantiation `Foo<A, B>` becomes an anonymous const block.
//! Inside it the generic parameter *names* are bound to the concrete
//! arguments (`type T = A;`, `const N: usize = 4;`) so the struct's field
//! types and deku attribute expressions can be copied verbatim instead of
//! substituted. Then a hidden non-generic "mirror" struct with the same
//! fields and `#[deku(...)]` attributes is defined and deku's derive runs on
//! it, and the deku traits for `Foo<A, B>` are implemented by delegating to
//! the mirror.
//!
//! The read mirror owns its fields; they get moved into `Foo<A, B>`. The write
//! mirror holds a reference to every field of `&Foo<A, B>` and works because
//! deku has a blanket `impl DekuWriter for &T`.

use proc_macro2::{Span, TokenStream};
use quote::{format_ident, quote};
use syn::{
    Attribute, Fields, GenericArgument, GenericParam, Ident, Index, ItemStruct, Lifetime,
    LifetimeParam, Path, PathArguments, Type,
};

use crate::Mode;
use crate::attrs::{self, MirrorKind};

/// Name of the helper `macro_rules!` emitted next to a `#[deku_generic]`
/// struct, through which `impl_deku_*!` reach the struct definition.
pub(crate) fn helper_ident(ident: &Ident) -> Ident {
    format_ident!("__deku_generic_{}", ident, span = Span::call_site())
}

/// Expansion of the `#[deku_generic(...)]` attribute.
pub(crate) fn attribute(item: ItemStruct, requests: Vec<(Mode, Path)>) -> syn::Result<TokenStream> {
    for field in item.fields.iter() {
        attrs::check_field_supported(&field.attrs)?;
    }
    attrs::parse_struct_attrs(&item.attrs)?;

    let captured = only_deku_attrs(&item);
    let real = without_deku_attrs(&item);
    let helper = helper_ident(&item.ident);
    let dg = crate::self_path();

    let mut out = quote! {
        #real

        #[doc(hidden)]
        #[allow(unused_macros)]
        macro_rules! #helper {
            ($($args:tt)*) => {
                #dg::__deku_generic_impl! { $($args)* ; #captured }
            };
        }
        #[doc(hidden)]
        #[allow(unused_imports)]
        pub(crate) use #helper;
    };

    for (mode, target) in &requests {
        out.extend(instantiate(*mode, target, &captured)?);
    }
    Ok(out)
}

fn only_deku_attrs(item: &ItemStruct) -> ItemStruct {
    let mut item = item.clone();
    item.attrs.retain(attrs::is_deku);
    for field in item.fields.iter_mut() {
        field.attrs.retain(attrs::is_deku);
    }
    item
}

fn without_deku_attrs(item: &ItemStruct) -> ItemStruct {
    let mut item = item.clone();
    item.attrs.retain(|a| !attrs::is_deku(a));
    for field in item.fields.iter_mut() {
        field.attrs.retain(|a| !attrs::is_deku(a));
    }
    item
}

/// One field of the struct plus the two names it goes by.
struct FieldInfo<'a> {
    ident: Option<&'a Ident>,
    index: Index,
    ty: &'a Type,
    attrs: &'a [Attribute],
    /// What deku's derive calls the field inside attribute expressions
    /// (`name`, or `field_0` for tuple structs).
    deku_name: String,
    /// Local binding used in the generated conversion code.
    local: Ident,
}

/// Generate all impls of `mode` for the concrete type `target`.
pub(crate) fn instantiate(
    mode: Mode,
    target: &Path,
    item: &ItemStruct,
) -> syn::Result<TokenStream> {
    let deku = crate::deku_path();

    let last = target
        .segments
        .last()
        .ok_or_else(|| syn::Error::new_spanned(target, "expected a type path"))?;
    if last.ident != item.ident {
        return Err(syn::Error::new_spanned(
            &last.ident,
            format!("expected `{}`, found `{}`", item.ident, last.ident),
        ));
    }

    // ---- generic arguments -> alias items -------------------------------

    let mut lifetime_args = Vec::new();
    let mut other_args = Vec::new();
    match &last.arguments {
        PathArguments::None => {}
        PathArguments::AngleBracketed(ab) => {
            for arg in &ab.args {
                match arg {
                    GenericArgument::Lifetime(lt) => lifetime_args.push(lt),
                    GenericArgument::Type(_) | GenericArgument::Const(_) => other_args.push(arg),
                    other => {
                        return Err(syn::Error::new_spanned(
                            other,
                            "unsupported generic argument",
                        ));
                    }
                }
            }
        }
        PathArguments::Parenthesized(p) => {
            return Err(syn::Error::new_spanned(
                p,
                "expected `<...>` generic arguments",
            ));
        }
    }

    let lifetime_params: Vec<&LifetimeParam> = item.generics.lifetimes().collect();
    let params: Vec<&GenericParam> = item
        .generics
        .params
        .iter()
        .filter(|p| !matches!(p, GenericParam::Lifetime(_)))
        .collect();

    if !lifetime_args.is_empty() && lifetime_args.len() != lifetime_params.len() {
        return Err(syn::Error::new_spanned(
            last,
            format!(
                "`{}` has {} lifetime parameter(s), but {} were given",
                item.ident,
                lifetime_params.len(),
                lifetime_args.len()
            ),
        ));
    }
    // The impls are generic over the struct's own lifetime names, so given
    // lifetimes must be those names (or `'_`).
    for (given, param) in lifetime_args.iter().zip(&lifetime_params) {
        if given.ident != "_" && given.ident != param.lifetime.ident {
            return Err(syn::Error::new_spanned(
                given,
                format!(
                    "lifetime must be written as `{}` (the struct's own name) or `'_`, or omitted",
                    param.lifetime
                ),
            ));
        }
    }
    // Normalise the target type to `Foo<'a, 'b, A, B>` with the struct's
    // lifetime names, whatever the caller wrote.
    let target = {
        let mut target = target.clone();
        let last = target.segments.last_mut().expect("checked above");
        let mut args: syn::punctuated::Punctuated<GenericArgument, syn::Token![,]> =
            syn::punctuated::Punctuated::new();
        for lp in &lifetime_params {
            args.push(GenericArgument::Lifetime(lp.lifetime.clone()));
        }
        for arg in &other_args {
            args.push((*arg).clone());
        }
        last.arguments = if args.is_empty() {
            PathArguments::None
        } else {
            PathArguments::AngleBracketed(syn::AngleBracketedGenericArguments {
                colon2_token: None,
                lt_token: Default::default(),
                args,
                gt_token: Default::default(),
            })
        };
        target
    };
    let target = &target;
    if other_args.len() > params.len() {
        return Err(syn::Error::new_spanned(
            last,
            format!(
                "`{}` has {} type/const parameter(s), but {} were given",
                item.ident,
                params.len(),
                other_args.len()
            ),
        ));
    }

    let mut outer_aliases = TokenStream::new();
    let mut inner_aliases = TokenStream::new();
    for (i, param) in params.iter().enumerate() {
        let arg = other_args.get(i).copied();
        match param {
            GenericParam::Type(tp) => {
                let name = &tp.ident;
                let ty = match (arg, &tp.default) {
                    (Some(GenericArgument::Type(t)), _) => quote!(#t),
                    (Some(other), _) => {
                        return Err(syn::Error::new_spanned(
                            other,
                            format!("expected a type for parameter `{name}`"),
                        ));
                    }
                    (None, Some(default)) => quote!(#default),
                    (None, None) => {
                        return Err(syn::Error::new_spanned(
                            last,
                            format!("missing argument for type parameter `{name}`"),
                        ));
                    }
                };
                let outer = format_ident!("__DekuGenericArg{}", i);
                outer_aliases.extend(quote! {
                    #[allow(dead_code, non_camel_case_types)]
                    type #outer = #ty;
                });
                inner_aliases.extend(quote! {
                    #[allow(dead_code, non_camel_case_types)]
                    type #name = #outer;
                });
            }
            GenericParam::Const(cp) => {
                let name = &cp.ident;
                let cty = &cp.ty;
                let value = match (arg, &cp.default) {
                    (Some(GenericArgument::Const(e)), _) => quote!(#e),
                    // A bare path such as `N` or `LEN` parses as a type.
                    (Some(GenericArgument::Type(t)), _) => quote!(#t),
                    (Some(other), _) => {
                        return Err(syn::Error::new_spanned(
                            other,
                            format!("expected a const for parameter `{name}`"),
                        ));
                    }
                    (None, Some(default)) => quote!(#default),
                    (None, None) => {
                        return Err(syn::Error::new_spanned(
                            last,
                            format!("missing argument for const parameter `{name}`"),
                        ));
                    }
                };
                let outer = format_ident!("__DEKU_GENERIC_ARG_{}", i);
                outer_aliases.extend(quote! {
                    #[allow(dead_code, non_upper_case_globals)]
                    const #outer: #cty = #value;
                });
                inner_aliases.extend(quote! {
                    #[allow(dead_code, non_upper_case_globals)]
                    const #name: #cty = #outer;
                });
            }
            GenericParam::Lifetime(_) => unreachable!("filtered out above"),
        }
    }

    // ---- shared pieces ------------------------------------------------------

    let fields: Vec<FieldInfo> = item
        .fields
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let local = match &f.ident {
                Some(id) => {
                    let mut id = id.clone();
                    id.set_span(Span::call_site());
                    id
                }
                None => format_ident!("__dg_field_{}", i),
            };
            FieldInfo {
                ident: f.ident.as_ref(),
                index: Index::from(i),
                ty: &f.ty,
                attrs: &f.attrs,
                deku_name: match &f.ident {
                    Some(id) => id.to_string(),
                    None => format!("field_{i}"),
                },
                local,
            }
        })
        .collect();
    let field_names: Vec<String> = fields.iter().map(|f| f.deku_name.clone()).collect();
    let locals: Vec<&Ident> = fields.iter().map(|f| &f.local).collect();

    let struct_deku_attrs: Vec<&Attribute> =
        item.attrs.iter().filter(|a| attrs::is_deku(a)).collect();
    let struct_attrs = attrs::parse_struct_attrs(&item.attrs)?;
    let where_clause = &item.generics.where_clause;

    let lifetime_names: Vec<&Lifetime> = lifetime_params.iter().map(|l| &l.lifetime).collect();
    let has_lifetimes = !lifetime_params.is_empty();

    let ctx_ty = match &struct_attrs.ctx {
        None => quote!(()),
        Some(types) if types.len() == 1 => {
            let t = &types[0];
            quote!(#t)
        }
        Some(types) => quote!((#(#types),*)),
    };
    // deku implements the `()` flavour of the traits when there is no ctx,
    // or when `ctx_default` supplies one.
    let has_unit_ctx = struct_attrs.ctx.is_none() || struct_attrs.ctx_default;
    let mut ctx_list = vec![ctx_ty];
    if struct_attrs.ctx.is_some() && struct_attrs.ctx_default {
        ctx_list.push(quote!(()));
    }

    let body = match mode {
        Mode::Read => emit_read(
            &deku,
            target,
            item,
            &fields,
            &locals,
            &field_names,
            &struct_deku_attrs,
            where_clause.as_ref(),
            &lifetime_params,
            &lifetime_names,
            has_lifetimes,
            &ctx_list,
            has_unit_ctx,
        )?,
        Mode::Write => emit_write(
            &deku,
            target,
            item,
            &fields,
            &field_names,
            &struct_deku_attrs,
            where_clause.as_ref(),
            &lifetime_params,
            &lifetime_names,
            has_lifetimes,
            &ctx_list,
            has_unit_ctx,
        )?,
    };

    Ok(quote! {
        const _: () = {
            #outer_aliases
            const _: () = {
                #inner_aliases
                #body
            };
        };
    })
}

#[allow(clippy::too_many_arguments)]
fn emit_read(
    deku: &TokenStream,
    target: &Path,
    item: &ItemStruct,
    fields: &[FieldInfo],
    locals: &[&Ident],
    field_names: &[String],
    struct_deku_attrs: &[&Attribute],
    where_clause: Option<&syn::WhereClause>,
    lifetime_params: &[&LifetimeParam],
    lifetime_names: &[&Lifetime],
    has_lifetimes: bool,
    ctx_list: &[TokenStream],
    has_unit_ctx: bool,
) -> syn::Result<TokenStream> {
    let mirror = format_ident!("__DekuGenericRead");

    let reader_lt: Lifetime = lifetime_names
        .first()
        .map(|l| (*l).clone())
        .unwrap_or_else(|| Lifetime::new("'__deku", Span::call_site()));
    let impl_generics = if has_lifetimes {
        quote!(<#(#lifetime_params),*>)
    } else {
        quote!(<'__deku>)
    };
    let mirror_generics = if has_lifetimes {
        quote!(<#(#lifetime_params),*>)
    } else {
        quote!()
    };
    let mirror_ty = if has_lifetimes {
        quote!(#mirror<#(#lifetime_names),*>)
    } else {
        quote!(#mirror)
    };

    // -- mirror struct
    let mut mirror_fields = Vec::new();
    for f in fields {
        let fattrs = field_attrs_with_phantom(f, MirrorKind::Read, field_names)?;
        let ty = f.ty;
        mirror_fields.push(match f.ident {
            Some(id) => quote!(#(#fattrs)* #id: #ty),
            None => quote!(#(#fattrs)* #ty),
        });
    }
    let mirror_def = mirror_struct(
        deku,
        "DekuRead",
        &mirror,
        &mirror_generics,
        where_clause,
        &item.fields,
        &mirror_fields,
        struct_deku_attrs,
    );

    // -- mirror -> Self conversion
    let convert = match &item.fields {
        Fields::Named(_) => quote! {
            let #mirror { #(#locals),* } = __dg_mirror;
            Self { #(#locals),* }
        },
        Fields::Unnamed(_) => quote! {
            let #mirror ( #(#locals),* ) = __dg_mirror;
            Self ( #(#locals),* )
        },
        Fields::Unit => quote! {
            let _ = __dg_mirror;
            Self
        },
    };

    let mut out = mirror_def;

    for ctx in ctx_list {
        out.extend(quote! {
            #[automatically_derived]
            impl #impl_generics #deku::DekuReader<#reader_lt, #ctx> for #target #where_clause {
                #[inline]
                fn from_reader_with_ctx<__R: #deku::no_std_io::Read + #deku::no_std_io::Seek>(
                    __deku_reader: &mut #deku::reader::Reader<__R>,
                    __deku_ctx: #ctx,
                ) -> ::core::result::Result<Self, #deku::DekuError> {
                    let __dg_mirror = <#mirror_ty as #deku::DekuReader<#reader_lt, #ctx>>::from_reader_with_ctx(
                        __deku_reader,
                        __deku_ctx,
                    )?;
                    ::core::result::Result::Ok({ #convert })
                }
            }
        });
    }

    if has_unit_ctx {
        out.extend(quote! {
            #[automatically_derived]
            impl #impl_generics #deku::DekuContainerRead<#reader_lt> for #target #where_clause {
                // Like deku's derive, accept any reader lifetime here rather
                // than the trait's, so callers can pass a local cursor.
                #[inline]
                fn from_reader<'__dg_r, __R: #deku::no_std_io::Read + #deku::no_std_io::Seek>(
                    __deku_input: (&'__dg_r mut __R, usize),
                ) -> ::core::result::Result<(usize, Self), #deku::DekuError> {
                    let __deku_reader = &mut #deku::reader::Reader::new(__deku_input.0);
                    if __deku_input.1 != 0 {
                        __deku_reader.skip_bits(__deku_input.1, #deku::ctx::Order::default())?;
                    }
                    let __deku_value = <Self as #deku::DekuReader<'_, ()>>::from_reader_with_ctx(__deku_reader, ())?;
                    ::core::result::Result::Ok((__deku_reader.bits_read, __deku_value))
                }

                #[inline]
                fn from_bytes(
                    __deku_input: (&#reader_lt [u8], usize),
                ) -> ::core::result::Result<((&#reader_lt [u8], usize), Self), #deku::DekuError> {
                    let mut __deku_cursor = #deku::no_std_io::Cursor::new(__deku_input.0);
                    let __deku_reader = &mut #deku::reader::Reader::new(&mut __deku_cursor);
                    if __deku_input.1 != 0 {
                        __deku_reader.skip_bits(__deku_input.1, #deku::ctx::Order::default())?;
                    }
                    let __deku_value = <Self as #deku::DekuReader<'_, ()>>::from_reader_with_ctx(__deku_reader, ())?;
                    let __deku_bits_read = __deku_reader.bits_read;
                    let __deku_idx = (__deku_bits_read - (__deku_bits_read % 8)) / 8;
                    let ::core::option::Option::Some(__deku_rest) = __deku_input.0.get(__deku_idx..) else {
                        return ::core::result::Result::Err(#deku::DekuError::Incomplete(
                            #deku::error::NeedSize::new(8 * (__deku_idx - __deku_input.0.len())),
                        ));
                    };
                    ::core::result::Result::Ok(((__deku_rest, __deku_bits_read % 8), __deku_value))
                }
            }

            #[automatically_derived]
            impl #impl_generics ::core::convert::TryFrom<&#reader_lt [u8]> for #target #where_clause {
                type Error = #deku::DekuError;

                #[inline]
                fn try_from(__deku_input: &#reader_lt [u8]) -> ::core::result::Result<Self, Self::Error> {
                    let __deku_total_len = __deku_input.len();
                    let mut __deku_cursor = #deku::no_std_io::Cursor::new(__deku_input);
                    let (__deku_bits_read, __deku_res) =
                        <Self as #deku::DekuContainerRead<'_>>::from_reader((&mut __deku_cursor, 0))?;
                    let __deku_bytes_read = __deku_bits_read / 8;
                    if __deku_bytes_read < __deku_total_len {
                        return ::core::result::Result::Err(#deku::deku_error!(
                            #deku::DekuError::Parse,
                            "Too much data",
                            "Read {} but total length was {}",
                            __deku_bytes_read,
                            __deku_total_len
                        ));
                    }
                    if __deku_bytes_read > __deku_total_len {
                        return ::core::result::Result::Err(#deku::DekuError::Incomplete(
                            #deku::error::NeedSize::new(__deku_bits_read - (__deku_total_len * 8)),
                        ));
                    }
                    ::core::result::Result::Ok(__deku_res)
                }
            }
        });
    }

    Ok(out)
}

#[allow(clippy::too_many_arguments)]
fn emit_write(
    deku: &TokenStream,
    target: &Path,
    item: &ItemStruct,
    fields: &[FieldInfo],
    field_names: &[String],
    struct_deku_attrs: &[&Attribute],
    where_clause: Option<&syn::WhereClause>,
    lifetime_params: &[&LifetimeParam],
    lifetime_names: &[&Lifetime],
    has_lifetimes: bool,
    ctx_list: &[TokenStream],
    has_unit_ctx: bool,
) -> syn::Result<TokenStream> {
    let mirror = format_ident!("__DekuGenericWrite");
    let ref_lt = Lifetime::new("'__dg_ref", Span::call_site());

    let impl_generics = if has_lifetimes {
        quote!(<#(#lifetime_params),*>)
    } else {
        quote!()
    };
    let mirror_generics = quote!(<#ref_lt, #(#lifetime_params),*>);
    let _ = lifetime_names;

    // -- mirror struct borrowing every field
    let mut mirror_fields = Vec::new();
    for f in fields {
        let fattrs = field_attrs_with_phantom(f, MirrorKind::Write, field_names)?;
        let ty = f.ty;
        mirror_fields.push(match f.ident {
            Some(id) => quote!(#(#fattrs)* #id: &#ref_lt #ty),
            None => quote!(#(#fattrs)* &#ref_lt #ty),
        });
    }
    let mirror_def = mirror_struct(
        deku,
        "DekuWrite",
        &mirror,
        &mirror_generics,
        where_clause,
        &item.fields,
        &mirror_fields,
        struct_deku_attrs,
    );

    // -- &Self -> mirror
    let construct = match &item.fields {
        Fields::Named(_) => {
            let idents = fields.iter().map(|f| f.ident.expect("named field"));
            quote!(#mirror { #(#idents: &self.#idents),* })
        }
        Fields::Unnamed(_) => {
            let indices = fields.iter().map(|f| &f.index);
            quote!(#mirror ( #(&self.#indices),* ))
        }
        Fields::Unit => quote!(#mirror),
    };

    // -- DekuUpdate on the real struct
    let mut updates = Vec::new();
    for f in fields {
        if let Some(expr) = attrs::field_update(f.attrs)? {
            let access = match f.ident {
                Some(id) => quote!(#id),
                None => {
                    let idx = &f.index;
                    quote!(#idx)
                }
            };
            updates.push(quote! {
                self.#access = (#expr).try_into()?;
            });
        }
    }

    let mut out = mirror_def;

    for ctx in ctx_list {
        out.extend(quote! {
            #[automatically_derived]
            impl #impl_generics #deku::DekuWriter<#ctx> for #target #where_clause {
                #[inline]
                fn to_writer<__W: #deku::no_std_io::Write + #deku::no_std_io::Seek>(
                    &self,
                    __deku_writer: &mut #deku::writer::Writer<__W>,
                    __deku_ctx: #ctx,
                ) -> ::core::result::Result<(), #deku::DekuError> {
                    let __dg_mirror = #construct;
                    #deku::DekuWriter::<#ctx>::to_writer(&__dg_mirror, __deku_writer, __deku_ctx)
                }
            }
        });
    }

    out.extend(quote! {
        #[automatically_derived]
        impl #impl_generics #deku::DekuUpdate for #target #where_clause {
            #[inline]
            fn update(&mut self) -> ::core::result::Result<(), #deku::DekuError> {
                #[allow(unused_imports)]
                use ::core::convert::TryInto;
                #(#updates)*
                ::core::result::Result::Ok(())
            }
        }
    });

    if has_unit_ctx {
        out.extend(quote! {
            #[automatically_derived]
            impl #impl_generics #deku::DekuContainerWrite for #target #where_clause {}

            const _: () = {
                extern crate alloc;

                #[automatically_derived]
                impl #impl_generics ::core::convert::TryFrom<#target> for alloc::vec::Vec<u8> #where_clause {
                    type Error = #deku::DekuError;

                    #[inline]
                    fn try_from(__deku_input: #target) -> ::core::result::Result<Self, Self::Error> {
                        #deku::DekuContainerWrite::to_bytes(&__deku_input)
                    }
                }
            };
        });
    }

    Ok(out)
}

/// Mirror attributes for a field. deku has no impls for `PhantomData`, so a
/// `PhantomData<..>` field without any `#[deku(...)]` attribute of its own is
/// treated as `#[deku(skip)]`: nothing is read or written for it.
fn field_attrs_with_phantom(
    f: &FieldInfo,
    kind: MirrorKind,
    field_names: &[String],
) -> syn::Result<Vec<Attribute>> {
    let mut fattrs = attrs::mirror_field_attrs(f.attrs, kind, field_names)?;
    if fattrs.is_empty() && is_phantom_data(f.ty) {
        fattrs.push(syn::parse_quote!(#[deku(skip)]));
    }
    Ok(fattrs)
}

fn is_phantom_data(ty: &Type) -> bool {
    match ty {
        Type::Path(p) => p
            .path
            .segments
            .last()
            .is_some_and(|s| s.ident == "PhantomData"),
        Type::Group(g) => is_phantom_data(&g.elem),
        Type::Paren(p) => is_phantom_data(&p.elem),
        _ => false,
    }
}

/// `#[derive(deku::<derive>)] struct <name><generics> where .. { fields }`
#[allow(clippy::too_many_arguments)]
fn mirror_struct(
    deku: &TokenStream,
    derive: &str,
    name: &Ident,
    generics: &TokenStream,
    where_clause: Option<&syn::WhereClause>,
    shape: &Fields,
    fields: &[TokenStream],
    struct_deku_attrs: &[&Attribute],
) -> TokenStream {
    let derive = format_ident!("{}", derive);
    let head = quote! {
        #[derive(#deku::#derive)]
        #(#struct_deku_attrs)*
        #[allow(dead_code, non_camel_case_types, non_snake_case, clippy::all)]
    };
    match shape {
        Fields::Named(_) => quote! {
            #head
            struct #name #generics #where_clause { #(#fields,)* }
        },
        Fields::Unnamed(_) => quote! {
            #head
            struct #name #generics ( #(#fields,)* ) #where_clause;
        },
        Fields::Unit => quote! {
            #head
            struct #name #generics #where_clause;
        },
    }
}
