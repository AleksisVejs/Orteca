//! What a source file defines and what it points at, for ranking the files a
//! task is about. Pure: source text in, facts out. No I/O, no model call.
//!
//! A use is a symbol name (`Quote`) or, when it contains a `/`, a path spec
//! (`./lib/api`, `resources/views/mail/quote.blade.php`, `lang/*/shop.php`).
//! Names resolve to files at query time, through whichever files define them.

use std::sync::OnceLock;
use streaming_iterator::StreamingIterator;
use tree_sitter::{Language, Node, Parser, Query, QueryCursor};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Php,
    Ts,
    Tsx,
    Js,
    Rust,
    /// A single-file component: its `<script>` blocks are parsed as TypeScript.
    Vue,
}

impl Lang {
    /// By extension. Blade templates are PHP files that are mostly HTML, so
    /// they are left out: a `view()` call points at them by path instead.
    pub fn of(path: &str) -> Option<Lang> {
        let lower = path.to_ascii_lowercase();
        if lower.ends_with(".blade.php") {
            return None;
        }
        Some(match lower.rsplit_once('.')?.1 {
            "php" => Lang::Php,
            "ts" | "mts" | "cts" => Lang::Ts,
            "tsx" => Lang::Tsx,
            "js" | "mjs" | "cjs" | "jsx" => Lang::Js,
            "rs" => Lang::Rust,
            "vue" => Lang::Vue,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Lang::Php => "php",
            Lang::Ts => "ts",
            Lang::Tsx => "tsx",
            Lang::Js => "js",
            Lang::Rust => "rust",
            Lang::Vue => "vue",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    pub name: String,
    /// `class`, `interface`, `trait`, `enum`, `function` or `method`.
    pub kind: String,
    /// 1-based.
    pub line: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileFacts {
    pub defines: Vec<Symbol>,
    /// Sorted, no duplicates.
    pub uses: Vec<String>,
}

const PHP: &str = r#"
(class_declaration name: (name) @class)
(interface_declaration name: (name) @interface)
(trait_declaration name: (name) @trait)
(enum_declaration name: (name) @enum)
(function_definition name: (name) @function)
(method_declaration name: (name) @method)

(namespace_use_clause . [(name) (qualified_name)] @use)
(base_clause [(name) (qualified_name)] @use)
(class_interface_clause [(name) (qualified_name)] @use)
(object_creation_expression [(name) (qualified_name)] @use)
(scoped_call_expression scope: [(name) (qualified_name)] @use)
(class_constant_access_expression . [(name) (qualified_name)] @use)

(string (string_content) @str)
(encapsed_string (string_content) @str)
(function_call_expression
  function: (name) @fn
  arguments: (arguments . (argument (string (string_content) @arg))))
(member_call_expression
  name: (name) @fn
  arguments: (arguments . (argument (string (string_content) @arg))))
(argument name: (name) @fn (string (string_content) @arg))
"#;

const JS: &str = r#"
(class_declaration name: (_) @class)
(function_declaration name: (identifier) @function)
(generator_function_declaration name: (identifier) @function)
(method_definition name: (property_identifier) @method)
(variable_declarator
  name: (identifier) @function
  value: [(arrow_function) (function_expression)])

(string (string_fragment) @str)
(template_string (string_fragment) @str)
(import_statement source: (string (string_fragment) @use))
(import_specifier name: (identifier) @use)
(import_clause (identifier) @use)
(call_expression
  function: (import)
  arguments: (arguments (string (string_fragment) @use)))
"#;

const TS_EXTRA: &str = r#"
(abstract_class_declaration name: (_) @class)
(interface_declaration name: (_) @interface)
(enum_declaration name: (_) @enum)
"#;

const RUST: &str = r#"
(struct_item name: (type_identifier) @class)
(enum_item name: (type_identifier) @enum)
(trait_item name: (type_identifier) @interface)
(function_item name: (identifier) @function)
(function_signature_item name: (identifier) @function)

(use_declaration argument: (_) @use)
(scoped_identifier path: (identifier) @use)
(scoped_type_identifier path: (identifier) @use)
"#;

struct Grammar {
    language: Language,
    query: Query,
}

fn grammar(lang: Lang) -> &'static Grammar {
    fn build(language: Language, source: &str) -> Grammar {
        let query = Query::new(&language, source).expect("code map query compiles");
        Grammar { language, query }
    }
    static PHP_G: OnceLock<Grammar> = OnceLock::new();
    static TS_G: OnceLock<Grammar> = OnceLock::new();
    static TSX_G: OnceLock<Grammar> = OnceLock::new();
    static JS_G: OnceLock<Grammar> = OnceLock::new();
    static RUST_G: OnceLock<Grammar> = OnceLock::new();
    match lang {
        Lang::Php => PHP_G.get_or_init(|| build(tree_sitter_php::LANGUAGE_PHP.into(), PHP)),
        Lang::Ts | Lang::Vue => TS_G.get_or_init(|| {
            build(tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(), &format!("{JS}{TS_EXTRA}"))
        }),
        Lang::Tsx => TSX_G.get_or_init(|| {
            build(tree_sitter_typescript::LANGUAGE_TSX.into(), &format!("{JS}{TS_EXTRA}"))
        }),
        Lang::Js => JS_G.get_or_init(|| build(tree_sitter_javascript::LANGUAGE.into(), JS)),
        Lang::Rust => RUST_G.get_or_init(|| build(tree_sitter_rust::LANGUAGE.into(), RUST)),
    }
}

pub fn parse(lang: Lang, source: &str) -> FileFacts {
    let mut facts = FileFacts::default();
    if lang == Lang::Vue {
        for (offset, script) in vue_scripts(source) {
            collect(Lang::Ts, script, offset, &mut facts);
        }
    } else {
        collect(lang, source, 0, &mut facts);
    }
    facts.defines.sort_by_key(|s| s.line);
    facts.defines.dedup();
    facts.uses.sort();
    facts.uses.dedup();
    facts
}

/// Each `<script>` block's body and the line it starts on, 0-based.
fn vue_scripts(source: &str) -> Vec<(u32, &str)> {
    let mut out = Vec::new();
    let mut rest = 0;
    while let Some(open) = source[rest..].find("<script") {
        let Some(body) = source[rest + open..].find('>').map(|i| rest + open + i + 1) else {
            break;
        };
        let Some(end) = source[body..].find("</script>").map(|i| body + i) else {
            break;
        };
        out.push((source[..body].matches('\n').count() as u32, &source[body..end]));
        rest = end;
    }
    out
}

fn collect(lang: Lang, source: &str, line_offset: u32, facts: &mut FileFacts) {
    let g = grammar(lang);
    let mut parser = Parser::new();
    if parser.set_language(&g.language).is_err() {
        return;
    }
    let Some(tree) = parser.parse(source, None) else {
        return;
    };
    let bytes = source.as_bytes();
    let names = g.query.capture_names();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&g.query, tree.root_node(), bytes);
    while let Some(m) = matches.next() {
        let mut call: Option<&str> = None;
        let mut arg: Option<&str> = None;
        for c in m.captures {
            let text = c.node.utf8_text(bytes).unwrap_or_default();
            match names[c.index as usize] {
                "use" if lang == Lang::Rust => rust_use(c.node, bytes, &mut facts.uses),
                "use" => push_use(lang, text, &mut facts.uses),
                "str" => {
                    if let Some(controller) = controller_action(text) {
                        facts.uses.push(controller.to_string());
                    }
                    if let Some(url) = url_key(text) {
                        facts.uses.push(format!("url:{url}"));
                    }
                }
                "fn" => call = Some(text),
                "arg" => arg = Some(text),
                kind => facts.defines.push(Symbol {
                    name: text.to_string(),
                    kind: symbol_kind(kind, c.node).to_string(),
                    line: c.node.start_position().row as u32 + 1 + line_offset,
                }),
            }
        }
        if let (Some(call), Some(arg)) = (call, arg) {
            facts.uses.extend(laravel_path(call, arg));
        }
    }
}

fn symbol_kind(kind: &str, node: Node) -> &'static str {
    // A Rust fn inside an `impl` or `trait` body is a method.
    let in_body = node
        .parent()
        .and_then(|item| item.parent())
        .is_some_and(|body| body.kind() == "declaration_list");
    match kind {
        "class" => "class",
        "interface" => "interface",
        "trait" => "trait",
        "enum" => "enum",
        "method" => "method",
        _ if in_body => "method",
        _ => "function",
    }
}

fn push_use(lang: Lang, text: &str, uses: &mut Vec<String>) {
    let name = if lang == Lang::Php {
        text.rsplit('\\').next().unwrap_or(text).trim()
    } else {
        text.trim()
    };
    if !name.is_empty() && !matches!(name, "self" | "static" | "parent") {
        uses.push(name.to_string());
    }
}

/// Every name a Rust `use` tree brings in, `crate`, `self` and `super` aside.
fn rust_use(node: Node, bytes: &[u8], uses: &mut Vec<String>) {
    if node.kind() == "use_declaration" {
        return;
    }
    if matches!(node.kind(), "identifier" | "type_identifier") {
        let name = node.utf8_text(bytes).unwrap_or_default();
        if !matches!(name, "crate" | "self" | "super" | "std") {
            uses.push(name.to_string());
        }
        return;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        rust_use(child, bytes, uses);
    }
}

/// A URL path in a string, as `api/quotes/*`: query dropped, placeholders
/// (`{quote}`, `:id`, `$id`, numbers) as `*`. Tests and clients call the
/// endpoints a prompt names, so this is how they are found.
pub fn url_key(text: &str) -> Option<String> {
    let path = text.strip_prefix('/')?.split(['?', '#']).next()?;
    let segments: Vec<String> = path
        .split('/')
        .filter(|s| !s.is_empty())
        .map(|s| {
            let dynamic = s.starts_with(['{', ':', '$']) || s.chars().all(|c| c.is_ascii_digit());
            if dynamic { "*".to_string() } else { s.to_ascii_lowercase() }
        })
        .collect();
    let plain = |s: &String| s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '*'));
    (!segments.is_empty() && segments.iter().any(|s| s != "*") && segments.iter().all(plain))
        .then(|| segments.join("/"))
}

/// `'App\Http\Controllers\QuoteController@show'` names `QuoteController`.
fn controller_action(text: &str) -> Option<&str> {
    let (class, method) = text.split_once('@')?;
    let class = class.rsplit('\\').next()?;
    let ident = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    (class.ends_with("Controller") && ident(class) && ident(method)).then_some(class)
}

/// The file a Laravel helper's string points at: `view('mail.quote')` is
/// `resources/views/mail/quote.blade.php`, `__('shop.title')` is
/// `lang/*/shop.php`. A few string rules, no framework model.
fn laravel_path(call: &str, arg: &str) -> Option<String> {
    let dotted = |s: &str| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    };
    match call {
        "view" | "markdown" | "text" if dotted(arg) => {
            Some(format!("resources/views/{}.blade.php", arg.replace('.', "/")))
        }
        "__" | "trans" | "trans_choice" => {
            let (file, _) = arg.split_once('.')?;
            dotted(file).then(|| format!("lang/*/{file}.php"))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn defs(facts: &FileFacts) -> Vec<(&str, &str, u32)> {
        facts
            .defines
            .iter()
            .map(|s| (s.name.as_str(), s.kind.as_str(), s.line))
            .collect()
    }

    #[test]
    fn php_classes_methods_and_what_they_point_at() {
        let src = r#"<?php
namespace App\Http\Controllers\Api;

use App\Models\Quote;
use App\Http\Requests\StoreQuoteRequest as Store;

class QuoteController extends Controller implements HasMiddleware
{
    public function show(Quote $quote)
    {
        $q = Quote::findOrFail(1);
        $mail = new \App\Mail\QuoteSent($q);
        static::boot();
        return view('mail.quote_sent', ['title' => __('shop.title')]);
    }

    public function content(): Content
    {
        return new Content(markdown: 'mail.quote-reminder');
    }
}

interface Priced {}
trait HasTotals { function total() {} }
enum Status: string { case Draft = 'draft'; }
function helper() {}
"#;
        let facts = parse(Lang::Php, src);
        assert_eq!(
            defs(&facts),
            vec![
                ("QuoteController", "class", 7),
                ("show", "method", 9),
                ("content", "method", 17),
                ("Priced", "interface", 23),
                ("HasTotals", "trait", 24),
                ("total", "method", 24),
                ("Status", "enum", 25),
                ("helper", "function", 26),
            ]
        );
        assert_eq!(
            facts.uses,
            vec![
                "Content",
                "Controller",
                "HasMiddleware",
                "Quote",
                "QuoteSent",
                "StoreQuoteRequest",
                "lang/*/shop.php",
                "resources/views/mail/quote-reminder.blade.php",
                "resources/views/mail/quote_sent.blade.php",
            ]
        );
    }

    #[test]
    fn php_routes_point_at_their_controllers() {
        let src = r#"<?php
use Illuminate\Support\Facades\Route;
Route::get('/quotes', [QuoteController::class, 'index']);
Route::post('/orders', 'App\Http\Controllers\OrderController@store');
Route::get('/x', 'not a controller@all');
"#;
        let facts = parse(Lang::Php, src);
        assert_eq!(
            facts.uses,
            vec!["OrderController", "QuoteController", "Route", "url:orders", "url:quotes", "url:x"]
        );
        assert!(facts.defines.is_empty());
    }

    #[test]
    fn url_strings_are_keyed_without_their_placeholders() {
        assert_eq!(url_key("/api/quotes/{quote}/cancel?x=1").as_deref(), Some("api/quotes/*/cancel"));
        assert_eq!(url_key("/api/orders/42").as_deref(), Some("api/orders/*"));
        assert_eq!(url_key("/api/conversations/").as_deref(), Some("api/conversations"));
        assert_eq!(url_key("/"), None);
        assert_eq!(url_key("/{id}"), None);
        assert_eq!(url_key("api/quotes"), None, "not a path without its slash");
        assert_eq!(url_key("/usr/bin and more"), None);
        let js = parse(Lang::Js, "api.get(`/quotes/${id}`); fetch('/api/blog/posts');");
        assert_eq!(js.uses, vec!["url:api/blog/posts", "url:quotes"]);
    }

    #[test]
    fn typescript_definitions_and_imports() {
        let src = r#"import { ref, computed as c } from 'vue';
import api from '@/lib/api';
import type { Quote } from './types';

export interface QuoteRow { id: number }
export enum Tab { Open }
export abstract class Base {}
export class QuoteStore extends Base {
  load() {}
}
export function formatTotal(n: number) { return n; }
const toCents = (n: number) => n * 100;
const page = () => import('./views/QuotePage.vue');
"#;
        let facts = parse(Lang::Ts, src);
        assert_eq!(
            defs(&facts),
            vec![
                ("QuoteRow", "interface", 5),
                ("Tab", "enum", 6),
                ("Base", "class", 7),
                ("QuoteStore", "class", 8),
                ("load", "method", 9),
                ("formatTotal", "function", 11),
                ("toCents", "function", 12),
                ("page", "function", 13),
            ]
        );
        assert_eq!(
            facts.uses,
            vec!["./types", "./views/QuotePage.vue", "@/lib/api", "Quote", "api", "computed", "ref", "vue"]
        );
    }

    #[test]
    fn javascript_parses_without_typescript_rules() {
        let src = "import { fetchQuote } from './api.js';\nexport function show() {}\nclass A { run() {} }\n";
        let facts = parse(Lang::Js, src);
        assert_eq!(defs(&facts), vec![("show", "function", 2), ("A", "class", 3), ("run", "method", 3)]);
        assert_eq!(facts.uses, vec!["./api.js", "fetchQuote"]);
    }

    #[test]
    fn vue_script_blocks_keep_their_file_lines() {
        let src = r#"<template>
  <QuoteCard :quote="q" />
</template>

<script setup lang="ts">
import QuoteCard from '@/components/QuoteCard.vue';
function reload() {}
</script>
"#;
        let facts = parse(Lang::Vue, src);
        assert_eq!(defs(&facts), vec![("reload", "function", 7)]);
        assert_eq!(facts.uses, vec!["@/components/QuoteCard.vue", "QuoteCard"]);
    }

    #[test]
    fn rust_items_and_use_paths() {
        let src = r#"use crate::routing::{self, Mode, RepoSignals};
use std::collections::HashMap;

pub struct Store { conn: u8 }
enum Kind { A }
trait Parse { fn parse(&self); }
impl Store {
    fn open() -> Self { project::git_state(); todo!() }
}
fn main() {}
"#;
        let facts = parse(Lang::Rust, src);
        assert_eq!(
            defs(&facts),
            vec![
                ("Store", "class", 4),
                ("Kind", "enum", 5),
                ("Parse", "interface", 6),
                ("parse", "method", 6),
                ("open", "method", 8),
                ("main", "function", 10),
            ]
        );
        assert_eq!(
            facts.uses,
            vec!["HashMap", "Mode", "RepoSignals", "collections", "project", "routing"]
        );
    }

    #[test]
    fn languages_by_extension() {
        assert_eq!(Lang::of("app/Models/Quote.php"), Some(Lang::Php));
        assert_eq!(Lang::of("resources/views/mail/x.blade.php"), None);
        assert_eq!(Lang::of("src/App.vue"), Some(Lang::Vue));
        assert_eq!(Lang::of("src/main.TSX"), Some(Lang::Tsx));
        assert_eq!(Lang::of("README.md"), None);
        assert_eq!(Lang::of("Makefile"), None);
    }
}
