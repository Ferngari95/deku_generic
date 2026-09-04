//! Handling of `#[deku(...)]` attributes when building the mirror structs.

use proc_macro2::{TokenStream, TokenTree};
use syn::punctuated::Punctuated;
use syn::{Attribute, Expr, ExprLit, FnArg, Lit, LitStr, Meta, Token, Type};

/// Whether the mirror is used for reading (owned fields) or writing
/// (fields are shared references into the real struct).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum MirrorKind {
    Read,
    Write,
}

pub(crate) fn is_deku(attr: &Attribute) -> bool {
    attr.path().is_ident("deku")
}

fn parse_metas(attr: &Attribute) -> syn::Result<Punctuated<Meta, Token![,]>> {
    attr.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)
}

fn meta_name(meta: &Meta) -> String {
    meta.path()
        .get_ident()
        .map(ToString::to_string)
        .unwrap_or_default()
}

fn str_value(meta: &Meta) -> Option<&LitStr> {
    match meta {
        Meta::NameValue(nv) => match &nv.value {
            Expr::Lit(ExprLit {
                lit: Lit::Str(s), ..
            }) => Some(s),
            _ => None,
        },
        _ => None,
    }
}

/// Struct-level deku settings that change the shape of the generated impls.
pub(crate) struct StructAttrs {
    /// Types of the `ctx = "a: A, b: B"` arguments, if any.
    pub ctx: Option<Vec<Type>>,
    /// Whether `ctx_default = "..."` is present.
    pub ctx_default: bool,
}

pub(crate) fn parse_struct_attrs(attrs: &[Attribute]) -> syn::Result<StructAttrs> {
    let mut out = StructAttrs {
        ctx: None,
        ctx_default: false,
    };
    for attr in attrs.iter().filter(|a| is_deku(a)) {
        for meta in parse_metas(attr)? {
            match meta_name(&meta).as_str() {
                "ctx" => {
                    let Some(lit) = str_value(&meta) else {
                        return Err(syn::Error::new_spanned(meta, "`ctx` expects a string"));
                    };
                    let args = lit.parse_with(Punctuated::<FnArg, Token![,]>::parse_terminated)?;
                    let mut types = Vec::new();
                    for arg in args {
                        match arg {
                            FnArg::Typed(pt) => types.push(*pt.ty),
                            FnArg::Receiver(r) => {
                                return Err(syn::Error::new_spanned(
                                    r,
                                    "`self` is not allowed in a deku `ctx`",
                                ));
                            }
                        }
                    }
                    out.ctx = Some(types);
                }
                "ctx_default" => out.ctx_default = true,
                "id_type" | "id" => {
                    return Err(syn::Error::new_spanned(
                        meta,
                        "deku_generic only supports structs; enum attributes are not allowed",
                    ));
                }
                _ => {}
            }
        }
    }
    Ok(out)
}

/// Error if the field is `#[deku(temp)]`, which `deku_generic` cannot support
/// because it changes the field list of the real struct.
pub(crate) fn check_field_supported(attrs: &[Attribute]) -> syn::Result<()> {
    for attr in attrs.iter().filter(|a| is_deku(a)) {
        for meta in parse_metas(attr)? {
            if meta_name(&meta) == "temp" {
                return Err(syn::Error::new_spanned(
                    meta,
                    "`#[deku(temp)]` fields are not supported by deku_generic",
                ));
            }
        }
    }
    Ok(())
}

/// The `update = "..."` expression of a field, if present.
pub(crate) fn field_update(attrs: &[Attribute]) -> syn::Result<Option<TokenStream>> {
    for attr in attrs.iter().filter(|a| is_deku(a)) {
        for meta in parse_metas(attr)? {
            if meta_name(&meta) == "update" {
                let Some(lit) = str_value(&meta) else {
                    return Err(syn::Error::new_spanned(meta, "`update` expects a string"));
                };
                return Ok(Some(lit.parse::<TokenStream>()?));
            }
        }
    }
    Ok(None)
}

/// Attributes whose string value is an expression evaluated while writing,
/// in which sibling fields are visible as `&T` locals.
const WRITE_EXPR_ATTRS: &[&str] = &[
    "writer",
    "cond",
    "assert",
    "bits",
    "bytes",
    "pad_bits_before",
    "pad_bytes_before",
    "pad_bits_after",
    "pad_bytes_after",
    "seek_from_current",
    "seek_from_end",
    "seek_from_start",
];

/// Rewrite the deku attributes of one field for a mirror struct.
///
/// `update` is dropped in both cases because `DekuUpdate` is generated on
/// the real struct. For the write mirror, whose fields are references, deku's
/// derive would hand attribute expressions the sibling fields as `&&T`, so
/// every expression that mentions a field is wrapped in a block that rebinds
/// it to `&T` first. That is what the expression would see on a plain
/// `#[derive(DekuWrite)]`.
pub(crate) fn mirror_field_attrs(
    attrs: &[Attribute],
    kind: MirrorKind,
    field_names: &[String],
) -> syn::Result<Vec<Attribute>> {
    let mut out = Vec::new();
    for attr in attrs.iter().filter(|a| is_deku(a)) {
        let mut metas: Punctuated<Meta, Token![,]> = Punctuated::new();
        for meta in parse_metas(attr)? {
            let name = meta_name(&meta);
            if name == "update" {
                continue;
            }
            let meta = if kind == MirrorKind::Write {
                rewrite_for_write(meta, &name, field_names)?
            } else {
                meta
            };
            metas.push(meta);
        }
        if !metas.is_empty() {
            out.push(syn::parse_quote!(#[deku(#metas)]));
        }
    }
    Ok(out)
}

fn rewrite_for_write(meta: Meta, name: &str, field_names: &[String]) -> syn::Result<Meta> {
    let mut nv = match meta {
        Meta::NameValue(nv) => nv,
        other => return Ok(other),
    };
    let Expr::Lit(ExprLit {
        lit: Lit::Str(lit), ..
    }) = &nv.value
    else {
        return Ok(Meta::NameValue(nv));
    };
    let span = lit.span();
    let value = lit.value();

    let new_value = if WRITE_EXPR_ATTRS.contains(&name) {
        wrap_expr(&value, field_names, "")?
    } else if name == "assert_eq" {
        // deku emits `*(field) == (value)`; `field` is `&&T` on the mirror,
        // so `*(field)` is `&T` and the value must be a `&T` as well.
        format!("&({})", wrap_expr(&value, field_names, "")?)
    } else if name == "endian" && value != "big" && value != "little" {
        wrap_expr(&value, field_names, "")?
    } else if name == "ctx" {
        let exprs = lit.parse_with(Punctuated::<Expr, Token![,]>::parse_terminated)?;
        let mut parts = Vec::new();
        for e in exprs {
            let text = quote::quote!(#e).to_string();
            parts.push(wrap_expr(&text, field_names, "")?);
        }
        parts.join(", ")
    } else {
        return Ok(Meta::NameValue(nv));
    };

    nv.value = Expr::Lit(ExprLit {
        attrs: Vec::new(),
        lit: Lit::Str(LitStr::new(&new_value, span)),
    });
    Ok(Meta::NameValue(nv))
}

/// `expr` -> `prefix{ let a = *a; (expr) }` for every field `a` mentioned.
fn wrap_expr(expr: &str, field_names: &[String], prefix: &str) -> syn::Result<String> {
    let tokens: TokenStream = expr.parse()?;
    let mut used = Vec::new();
    collect_idents(&tokens, &mut used);
    let rebinds: Vec<String> = field_names
        .iter()
        .filter(|f| used.contains(f))
        .map(|f| format!("let {f} = *{f};"))
        .collect();
    if rebinds.is_empty() {
        return Ok(expr.to_owned());
    }
    Ok(format!("{prefix}{{ {} ({expr}) }}", rebinds.join(" ")))
}

fn collect_idents(tokens: &TokenStream, out: &mut Vec<String>) {
    for tt in tokens.clone() {
        match tt {
            TokenTree::Ident(i) => out.push(i.to_string()),
            TokenTree::Group(g) => collect_idents(&g.stream(), out),
            _ => {}
        }
    }
}
