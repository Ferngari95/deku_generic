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
use syn::punctuated::Punctuated;
use syn::{
    AngleBracketedGenericArguments, Attribute, ConstParam, Fields, GenericArgument, GenericParam,
    Ident, Index, ItemStruct, Lifetime, LifetimeParam, Path, PathArguments, PathSegment, Token,
    Type, TypeParam, WhereClause,
};

use crate::Mode;
use crate::attrs::{self, MirrorKind};

/// Name of the helper `macro_rules!` emitted next to a `#[deku_generic]`
/// struct, through which `impl_deku_*!` reach the struct definition.
pub(crate) fn helper_ident(ident: &Ident) -> Ident {
    format_ident!("__deku_generic_{}", ident, span = Span::call_site())
}

/// Expansion of the `#[deku_generic(...)]` attribute.
pub(crate) fn attribute(item: &ItemStruct, requests: &[(Mode, Path)]) -> syn::Result<TokenStream> {
    // Validate everything here rather than in the instantiations, so that
    // errors point at the attribute and not into a helper macro expansion.
    attrs::parse_struct_attrs(&item.attrs)?;
    let field_names: Vec<String> = item
        .fields
        .iter()
        .enumerate()
        .map(|(i, f)| FieldInfo::new(i, f).deku_name)
        .collect();
    for field in &item.fields {
        attrs::check_field_supported(&field.attrs)?;
        attrs::field_update(&field.attrs)?;
        for kind in [MirrorKind::Read, MirrorKind::Write] {
            attrs::mirror_field_attrs(&field.attrs, kind, &field_names)?;
        }
    }

    let captured = only_deku_attrs(item);
    let real = without_deku_attrs(item);
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

    for (mode, target) in requests {
        out.extend(instantiate(*mode, target, &captured)?);
    }
    Ok(out)
}

/// What `#[deku_generic]` emits when it has reported an error: the struct
/// itself, and a helper macro that expands to nothing so that
/// `impl_deku_*!` calls for it do not add a second error about a missing
/// macro.
pub(crate) fn fallback(item: &ItemStruct) -> TokenStream {
    let real = without_deku_attrs(item);
    let helper = helper_ident(&item.ident);
    quote! {
        #real

        #[doc(hidden)]
        #[allow(unused_macros)]
        macro_rules! #helper {
            ($($args:tt)*) => {};
        }
        #[doc(hidden)]
        #[allow(unused_imports)]
        pub(crate) use #helper;
    }
}

fn only_deku_attrs(item: &ItemStruct) -> ItemStruct {
    let mut item = item.clone();
    item.attrs.retain(attrs::is_deku);
    for field in &mut item.fields {
        field.attrs.retain(attrs::is_deku);
    }
    item
}

fn without_deku_attrs(item: &ItemStruct) -> ItemStruct {
    let mut item = item.clone();
    item.attrs.retain(|a| !attrs::is_deku(a));
    for field in &mut item.fields {
        field.attrs.retain(|a| !attrs::is_deku(a));
    }
    item
}

/// Generate all impls of `mode` for the concrete type `target`.
pub(crate) fn instantiate(
    mode: Mode,
    target: &Path,
    item: &ItemStruct,
) -> syn::Result<TokenStream> {
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

    let (lifetime_args, other_args) = split_args(last)?;
    let lifetime_params: Vec<&LifetimeParam> = item.generics.lifetimes().collect();
    check_lifetime_args(last, item, &lifetime_args, &lifetime_params)?;
    let target = normalise_target(target, &lifetime_params, &other_args);
    let (outer_aliases, inner_aliases) = alias_items(last, item, &other_args)?;

    let instance = Instance::new(target, item, lifetime_params)?;
    let body = match mode {
        Mode::Read => instance.read()?,
        Mode::Write => instance.write()?,
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

/// Split `Foo<'a, A, 4>` into its lifetime arguments and the rest.
fn split_args(last: &PathSegment) -> syn::Result<(Vec<&Lifetime>, Vec<&GenericArgument>)> {
    let mut lifetimes = Vec::new();
    let mut others = Vec::new();
    match &last.arguments {
        PathArguments::None => {}
        PathArguments::AngleBracketed(ab) => {
            for arg in &ab.args {
                match arg {
                    GenericArgument::Lifetime(lt) => lifetimes.push(lt),
                    GenericArgument::Type(_) | GenericArgument::Const(_) => others.push(arg),
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
    Ok((lifetimes, others))
}

/// The impls are generic over the struct's own lifetime names, so lifetimes
/// given by the caller must be those names (or `'_`), or be left out.
fn check_lifetime_args(
    last: &PathSegment,
    item: &ItemStruct,
    given: &[&Lifetime],
    params: &[&LifetimeParam],
) -> syn::Result<()> {
    if !given.is_empty() && given.len() != params.len() {
        return Err(syn::Error::new_spanned(
            last,
            format!(
                "`{}` has {} lifetime parameter(s), but {} were given",
                item.ident,
                params.len(),
                given.len()
            ),
        ));
    }
    for (given, param) in given.iter().zip(params) {
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
    Ok(())
}

/// `Foo<'a, 'b, A, B>` with the struct's lifetime names, whatever the caller
/// wrote.
fn normalise_target(
    target: &Path,
    lifetime_params: &[&LifetimeParam],
    other_args: &[&GenericArgument],
) -> Path {
    let mut args: Punctuated<GenericArgument, Token![,]> = Punctuated::new();
    for lp in lifetime_params {
        args.push(GenericArgument::Lifetime(lp.lifetime.clone()));
    }
    for arg in other_args {
        args.push((*arg).clone());
    }
    let arguments = if args.is_empty() {
        PathArguments::None
    } else {
        PathArguments::AngleBracketed(AngleBracketedGenericArguments {
            colon2_token: None,
            lt_token: syn::token::Lt::default(),
            args,
            gt_token: syn::token::Gt::default(),
        })
    };
    let mut target = target.clone();
    if let Some(last) = target.segments.last_mut() {
        last.arguments = arguments;
    }
    target
}

/// Alias items binding each type/const parameter name to its argument.
///
/// Goes through an intermediate name (`__DekuGenericArgN`) in an outer
/// block, because a direct `type State = State;` is a cycle whenever the
/// caller's argument happens to be spelled like the parameter.
fn alias_items(
    last: &PathSegment,
    item: &ItemStruct,
    other_args: &[&GenericArgument],
) -> syn::Result<(TokenStream, TokenStream)> {
    let param_count = item
        .generics
        .params
        .iter()
        .filter(|p| !matches!(p, GenericParam::Lifetime(_)))
        .count();
    if other_args.len() > param_count {
        return Err(syn::Error::new_spanned(
            last,
            format!(
                "`{}` has {} type/const parameter(s), but {} were given",
                item.ident,
                param_count,
                other_args.len()
            ),
        ));
    }

    let mut outer = TokenStream::new();
    let mut inner = TokenStream::new();
    let mut i = 0;
    for param in &item.generics.params {
        let arg = other_args.get(i).copied();
        match param {
            GenericParam::Lifetime(_) => continue,
            GenericParam::Type(tp) => {
                let ty = type_arg(last, tp, arg)?;
                let name = &tp.ident;
                let alias = format_ident!("__DekuGenericArg{}", i);
                outer.extend(quote! {
                    #[allow(dead_code, non_camel_case_types)]
                    type #alias = #ty;
                });
                inner.extend(quote! {
                    #[allow(dead_code, non_camel_case_types)]
                    type #name = #alias;
                });
            }
            GenericParam::Const(cp) => {
                let value = const_arg(last, cp, arg)?;
                let name = &cp.ident;
                let cty = &cp.ty;
                let alias = format_ident!("__DEKU_GENERIC_ARG_{}", i);
                outer.extend(quote! {
                    #[allow(dead_code, non_upper_case_globals)]
                    const #alias: #cty = #value;
                });
                inner.extend(quote! {
                    #[allow(dead_code, non_upper_case_globals)]
                    const #name: #cty = #alias;
                });
            }
        }
        i += 1;
    }
    Ok((outer, inner))
}

fn type_arg(
    last: &PathSegment,
    param: &TypeParam,
    arg: Option<&GenericArgument>,
) -> syn::Result<TokenStream> {
    let name = &param.ident;
    match (arg, &param.default) {
        (Some(GenericArgument::Type(t)), _) => Ok(quote!(#t)),
        (Some(other), _) => Err(syn::Error::new_spanned(
            other,
            format!("expected a type for parameter `{name}`"),
        )),
        (None, Some(default)) => Ok(quote!(#default)),
        (None, None) => Err(syn::Error::new_spanned(
            last,
            format!("missing argument for type parameter `{name}`"),
        )),
    }
}

fn const_arg(
    last: &PathSegment,
    param: &ConstParam,
    arg: Option<&GenericArgument>,
) -> syn::Result<TokenStream> {
    let name = &param.ident;
    match (arg, &param.default) {
        (Some(GenericArgument::Const(e)), _) => Ok(quote!(#e)),
        // A bare path such as `N` or `LEN` parses as a type.
        (Some(GenericArgument::Type(t)), _) => Ok(quote!(#t)),
        (Some(other), _) => Err(syn::Error::new_spanned(
            other,
            format!("expected a const for parameter `{name}`"),
        )),
        (None, Some(default)) => Ok(quote!(#default)),
        (None, None) => Err(syn::Error::new_spanned(
            last,
            format!("missing argument for const parameter `{name}`"),
        )),
    }
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

impl<'a> FieldInfo<'a> {
    fn new(i: usize, field: &'a syn::Field) -> Self {
        let local = if let Some(id) = &field.ident {
            let mut id = id.clone();
            id.set_span(Span::call_site());
            id
        } else {
            format_ident!("__dg_field_{}", i)
        };
        FieldInfo {
            ident: field.ident.as_ref(),
            index: Index::from(i),
            ty: &field.ty,
            attrs: &field.attrs,
            deku_name: field
                .ident
                .as_ref()
                .map_or_else(|| format!("field_{i}"), ToString::to_string),
            local,
        }
    }

    /// Attributes for the mirror copy of this field. deku has no impls for
    /// `PhantomData`, so a `PhantomData<..>` field without any `#[deku(...)]`
    /// attribute of its own is treated as `#[deku(skip)]`.
    fn mirror_attrs(
        &self,
        kind: MirrorKind,
        field_names: &[String],
    ) -> syn::Result<Vec<Attribute>> {
        let mut fattrs = attrs::mirror_field_attrs(self.attrs, kind, field_names)?;
        if fattrs.is_empty() && is_phantom_data(self.ty) {
            fattrs.push(syn::parse_quote!(#[deku(skip)]));
        }
        Ok(fattrs)
    }
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

/// Everything the read and write emitters need about one instantiation.
struct Instance<'a> {
    deku: TokenStream,
    target: Path,
    item: &'a ItemStruct,
    fields: Vec<FieldInfo<'a>>,
    field_names: Vec<String>,
    struct_deku_attrs: Vec<&'a Attribute>,
    where_clause: Option<&'a WhereClause>,
    lifetime_params: Vec<&'a LifetimeParam>,
    /// Ctx types deku implements the traits for: the declared `ctx`, plus
    /// `()` when there is a `ctx_default`.
    ctx_list: Vec<TokenStream>,
    /// Whether the `()` flavour exists, and with it the container impls.
    has_unit_ctx: bool,
}

impl<'a> Instance<'a> {
    fn new(
        target: Path,
        item: &'a ItemStruct,
        lifetime_params: Vec<&'a LifetimeParam>,
    ) -> syn::Result<Self> {
        let fields: Vec<FieldInfo<'a>> = item
            .fields
            .iter()
            .enumerate()
            .map(|(i, f)| FieldInfo::new(i, f))
            .collect();
        let field_names = fields.iter().map(|f| f.deku_name.clone()).collect();

        let struct_attrs = attrs::parse_struct_attrs(&item.attrs)?;
        let ctx_ty = match struct_attrs.ctx.as_deref() {
            None => quote!(()),
            Some([t]) => quote!(#t),
            Some(types) => quote!((#(#types),*)),
        };
        let has_unit_ctx = struct_attrs.ctx.is_none() || struct_attrs.ctx_default;
        let mut ctx_list = vec![ctx_ty];
        if struct_attrs.ctx.is_some() && struct_attrs.ctx_default {
            ctx_list.push(quote!(()));
        }

        Ok(Instance {
            deku: crate::deku_path(),
            target,
            item,
            fields,
            field_names,
            struct_deku_attrs: item.attrs.iter().filter(|a| attrs::is_deku(a)).collect(),
            where_clause: item.generics.where_clause.as_ref(),
            lifetime_params,
            ctx_list,
            has_unit_ctx,
        })
    }

    fn lifetime_names(&self) -> impl Iterator<Item = &'a Lifetime> + '_ {
        self.lifetime_params.iter().map(|l| &l.lifetime)
    }

    /// `#[derive(deku::<derive>)] struct <name><generics> where .. { fields }`
    fn mirror_struct(
        &self,
        derive: &str,
        name: &Ident,
        generics: &TokenStream,
        fields: &[TokenStream],
    ) -> TokenStream {
        let deku = &self.deku;
        let derive = format_ident!("{}", derive);
        let attrs = &self.struct_deku_attrs;
        let where_clause = self.where_clause;
        let head = quote! {
            #[derive(#deku::#derive)]
            #(#attrs)*
            #[allow(dead_code, non_camel_case_types, non_snake_case, clippy::all)]
        };
        match self.item.fields {
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

    /// Mirror struct plus `DekuReader`, `DekuContainerRead` and
    /// `TryFrom<&[u8]>` impls.
    fn read(&self) -> syn::Result<TokenStream> {
        let deku = &self.deku;
        let target = &self.target;
        let where_clause = self.where_clause;
        let mirror = format_ident!("__DekuGenericRead");

        let lifetime_params = &self.lifetime_params;
        let lifetime_names: Vec<&Lifetime> = self.lifetime_names().collect();
        let reader_lt = lifetime_names.first().map_or_else(
            || Lifetime::new("'__deku", Span::call_site()),
            |l| (*l).clone(),
        );
        let (impl_generics, mirror_generics, mirror_ty) = if lifetime_params.is_empty() {
            (quote!(<'__deku>), quote!(), quote!(#mirror))
        } else {
            (
                quote!(<#(#lifetime_params),*>),
                quote!(<#(#lifetime_params),*>),
                quote!(#mirror<#(#lifetime_names),*>),
            )
        };

        let mut mirror_fields = Vec::new();
        for f in &self.fields {
            let fattrs = f.mirror_attrs(MirrorKind::Read, &self.field_names)?;
            let ty = f.ty;
            mirror_fields.push(if let Some(id) = f.ident {
                quote!(#(#fattrs)* #id: #ty)
            } else {
                quote!(#(#fattrs)* #ty)
            });
        }
        let mut out = self.mirror_struct("DekuRead", &mirror, &mirror_generics, &mirror_fields);

        let locals: Vec<&Ident> = self.fields.iter().map(|f| &f.local).collect();
        let convert = match self.item.fields {
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

        for ctx in &self.ctx_list {
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

        if self.has_unit_ctx {
            out.extend(self.container_read(&impl_generics, &reader_lt));
        }
        Ok(out)
    }

    /// `DekuContainerRead` and `TryFrom<&[u8]>`, in terms of the
    /// `DekuReader<'_, ()>` impl. Same bodies as deku's derive.
    fn container_read(&self, impl_generics: &TokenStream, reader_lt: &Lifetime) -> TokenStream {
        let deku = &self.deku;
        let target = &self.target;
        let where_clause = self.where_clause;
        quote! {
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
        }
    }

    /// Mirror struct plus `DekuWriter`, `DekuUpdate`, `DekuContainerWrite`
    /// and `TryFrom<Self> for Vec<u8>` impls.
    fn write(&self) -> syn::Result<TokenStream> {
        let deku = &self.deku;
        let target = &self.target;
        let where_clause = self.where_clause;
        let mirror = format_ident!("__DekuGenericWrite");
        let ref_lt = Lifetime::new("'__dg_ref", Span::call_site());

        let lifetime_params = &self.lifetime_params;
        let impl_generics = if lifetime_params.is_empty() {
            quote!()
        } else {
            quote!(<#(#lifetime_params),*>)
        };
        let mirror_generics = quote!(<#ref_lt, #(#lifetime_params),*>);

        let mut mirror_fields = Vec::new();
        for f in &self.fields {
            let fattrs = f.mirror_attrs(MirrorKind::Write, &self.field_names)?;
            let ty = f.ty;
            mirror_fields.push(if let Some(id) = f.ident {
                quote!(#(#fattrs)* #id: &#ref_lt #ty)
            } else {
                quote!(#(#fattrs)* &#ref_lt #ty)
            });
        }
        let mut out = self.mirror_struct("DekuWrite", &mirror, &mirror_generics, &mirror_fields);

        let construct = match self.item.fields {
            Fields::Named(_) => {
                let idents = self.fields.iter().filter_map(|f| f.ident);
                quote!(#mirror { #(#idents: &self.#idents),* })
            }
            Fields::Unnamed(_) => {
                let indices = self.fields.iter().map(|f| &f.index);
                quote!(#mirror ( #(&self.#indices),* ))
            }
            Fields::Unit => quote!(#mirror),
        };

        for ctx in &self.ctx_list {
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

        out.extend(self.update_impl(&impl_generics)?);
        if self.has_unit_ctx {
            out.extend(self.container_write(&impl_generics));
        }
        Ok(out)
    }

    /// `DekuUpdate` on the real struct, from the fields' `update = ".."`.
    fn update_impl(&self, impl_generics: &TokenStream) -> syn::Result<TokenStream> {
        let deku = &self.deku;
        let target = &self.target;
        let where_clause = self.where_clause;

        let mut updates = Vec::new();
        for f in &self.fields {
            if let Some(expr) = attrs::field_update(f.attrs)? {
                let access = if let Some(id) = f.ident {
                    quote!(#id)
                } else {
                    let idx = &f.index;
                    quote!(#idx)
                };
                updates.push(quote! {
                    self.#access = (#expr).try_into()?;
                });
            }
        }

        Ok(quote! {
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
        })
    }

    /// `DekuContainerWrite` (all provided methods) and `TryFrom<Self> for Vec<u8>`.
    fn container_write(&self, impl_generics: &TokenStream) -> TokenStream {
        let deku = &self.deku;
        let target = &self.target;
        let where_clause = self.where_clause;
        quote! {
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
        }
    }
}
