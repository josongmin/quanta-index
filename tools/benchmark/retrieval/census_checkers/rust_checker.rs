//! Independent Rust declaration census over `syn` (not Tree-sitter).
//!
//! Reads one UTF-8 file path per stdin line and writes one JSON object per
//! line: `{"path", "declarations": [[name, kind, line, char_column]]}` or
//! `{"path", "error"}`. Lines are 1-based; columns count Unicode scalar values.
use std::io::{self, BufRead, Write};

use proc_macro2::Span;
use syn::visit::{self, Visit};

#[derive(Default)]
struct Census {
    rows: Vec<(String, &'static str, usize, usize)>,
}

impl Census {
    fn push(&mut self, ident: &syn::Ident, kind: &'static str) {
        let name = ident.to_string();
        if name == "_" {
            return;
        }
        let start = Span::start(&ident.span());
        self.rows.push((name, kind, start.line, start.column));
    }
}

macro_rules! named {
    ($method:ident, $ty:ty, $kind:literal, |$item:ident| $ident:expr) => {
        fn $method(&mut self, $item: &'ast $ty) {
            self.push($ident, $kind);
            visit::$method(self, $item);
        }
    };
}

impl<'ast> Visit<'ast> for Census {
    named!(visit_item_fn, syn::ItemFn, "fn", |i| &i.sig.ident);
    named!(visit_impl_item_fn, syn::ImplItemFn, "fn", |i| &i.sig.ident);
    named!(visit_trait_item_fn, syn::TraitItemFn, "fn", |i| &i.sig.ident);
    named!(visit_foreign_item_fn, syn::ForeignItemFn, "fn", |i| &i.sig.ident);
    named!(visit_item_struct, syn::ItemStruct, "struct", |i| &i.ident);
    named!(visit_item_enum, syn::ItemEnum, "enum", |i| &i.ident);
    named!(visit_item_union, syn::ItemUnion, "union", |i| &i.ident);
    named!(visit_item_trait, syn::ItemTrait, "trait", |i| &i.ident);
    named!(visit_item_type, syn::ItemType, "type", |i| &i.ident);
    named!(visit_impl_item_type, syn::ImplItemType, "type", |i| &i.ident);
    named!(visit_trait_item_type, syn::TraitItemType, "type", |i| &i.ident);
    named!(visit_foreign_item_type, syn::ForeignItemType, "type", |i| &i.ident);
    named!(visit_item_const, syn::ItemConst, "const", |i| &i.ident);
    named!(visit_impl_item_const, syn::ImplItemConst, "const", |i| &i.ident);
    named!(visit_trait_item_const, syn::TraitItemConst, "const", |i| &i.ident);
    named!(visit_item_static, syn::ItemStatic, "static", |i| &i.ident);
    named!(visit_foreign_item_static, syn::ForeignItemStatic, "static", |i| &i.ident);
    named!(visit_item_mod, syn::ItemMod, "mod", |i| &i.ident);

    fn visit_item_macro(&mut self, item: &'ast syn::ItemMacro) {
        if let Some(ident) = &item.ident {
            self.push(ident, "macro_rules");
        }
        visit::visit_item_macro(self, item);
    }
}

fn main() {
    let stdin = io::stdin();
    let mut stdout = io::BufWriter::new(io::stdout().lock());
    for line in stdin.lock().lines() {
        let path = line.expect("stdin path");
        let value = match std::fs::read_to_string(&path) {
            Err(error) => serde_json::json!({"path": path, "error": format!("read: {error}")}),
            Ok(text) => match syn::parse_file(&text) {
                Err(error) => serde_json::json!({"path": path, "error": format!("parse: {error}")}),
                Ok(file) => {
                    let mut census = Census::default();
                    census.visit_file(&file);
                    serde_json::json!({"path": path, "declarations": census.rows})
                }
            },
        };
        writeln!(stdout, "{value}").expect("stdout");
    }
}
