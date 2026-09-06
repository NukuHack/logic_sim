use proc_macro::TokenStream;
use proc_macro2::Span;
use quote::quote;
use syn::{parse_macro_input, Data, DeriveInput, Expr, Fields, Ident};

#[proc_macro_derive(ConstFromPrimitive)]
pub fn derive_const_from_primitive(input: TokenStream) -> TokenStream {
	let input = parse_macro_input!(input as DeriveInput);
	let name = &input.ident;

	// Find #[repr(...)] to know the primitive type.
	let repr: Ident = input
		.attrs
		.iter()
		.find(|attr| attr.path().is_ident("repr"))
		.and_then(|attr| attr.parse_args::<Ident>().ok())
		.unwrap_or_else(|| Ident::new("i32", Span::call_site()));

	let data_enum = match &input.data {
		Data::Enum(data_enum) => data_enum,
		_ => {
			return syn::Error::new(Span::call_site(), "ConstFromPrimitive only supports enums").to_compile_error().into();
		}
	};

	let mut variant_idents: Vec<Ident> = Vec::new();
	let mut variant_exprs: Vec<Expr> = Vec::new();
	let mut default_ident: Option<Ident> = None;

	for variant in &data_enum.variants {
		if !matches!(variant.fields, Fields::Unit) {
			return syn::Error::new_spanned(variant, "ConstFromPrimitive only supports fieldless (unit) variants").to_compile_error().into();
		}

		let (_, discriminant) =
			variant.discriminant.as_ref().unwrap_or_else(|| panic!("variant `{}` needs an explicit discriminant (e.g. `= 0`)", variant.ident));

		if variant.attrs.iter().any(|attr| attr.path().is_ident("default")) {
			default_ident = Some(variant.ident.clone());
		}

		variant_idents.push(variant.ident.clone());
		variant_exprs.push(discriminant.clone());
	}

	let default_ident = match default_ident {
		Some(ident) => ident,
		None => {
			return syn::Error::new(Span::call_site(), "ConstFromPrimitive requires one variant marked `#[default]`").to_compile_error().into();
		}
	};

	TokenStream::from(quote! {
		impl #name {
			#[allow(non_upper_case_globals)]
			pub const fn from_primitive(number: #repr) -> Self {
				#(
					const #variant_idents: #repr = #variant_exprs;
				)*
				match number {
					#( #variant_idents => Self::#variant_idents, )*
					_ => Self::#default_ident,
				}
			}
		}
	})
}
