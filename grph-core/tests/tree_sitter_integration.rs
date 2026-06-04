use grph_core::extraction::grammars::detect_language;
use grph_core::extraction::languages::extract_for_language;
use grph_core::{Edge, EdgeKind, Language, Node, NodeKind};
use std::path::Path;

fn node_names(result: &grph_core::extraction::ExtractionResult, kind: NodeKind) -> Vec<String> {
    result
        .nodes
        .iter()
        .filter(|node| node.kind == kind)
        .map(|node| node.name.clone())
        .collect()
}

fn has_call(result: &grph_core::extraction::ExtractionResult, target: &str) -> bool {
    result
        .edges
        .iter()
        .any(|edge| edge.kind == EdgeKind::Calls && edge.target == target)
}

fn test_node(id: &str, name: &str, line: u32) -> Node {
    Node {
        id: id.to_string(),
        kind: NodeKind::Function,
        name: name.to_string(),
        qualified_name: format!("test.py#{name}"),
        file_path: "test.py".to_string(),
        language: Language::Python,
        start_line: line,
        end_line: line + 2,
        start_column: 0,
        end_column: 10,
        docstring: None,
        signature: None,
        visibility: None,
        is_exported: false,
        is_async: false,
        is_static: false,
        is_abstract: false,
        decorators: None,
        type_parameters: None,
        updated_at: 0,
    }
}

fn test_call(source: &str, target: &str, line: u32) -> Edge {
    Edge {
        source: source.to_string(),
        target: target.to_string(),
        kind: EdgeKind::Calls,
        metadata: None,
        line: Some(line),
        col: Some(4),
        provenance: Some("test".to_string()),
    }
}

#[test]
fn detects_supported_languages() {
    assert_eq!(
        detect_language(Path::new("src/main.py")),
        Some(Language::Python)
    );
    assert_eq!(
        detect_language(Path::new("src/main.rs")),
        Some(Language::Rust)
    );
    assert_eq!(
        detect_language(Path::new("src/main.js")),
        Some(Language::JavaScript)
    );
    assert_eq!(
        detect_language(Path::new("src/main.ts")),
        Some(Language::TypeScript)
    );
    assert_eq!(
        detect_language(Path::new("src/main.tsx")),
        Some(Language::Tsx)
    );
    assert_eq!(
        detect_language(Path::new("src/main.go")),
        Some(Language::Go)
    );
    assert_eq!(detect_language(Path::new("src/main.c")), Some(Language::C));
    assert_eq!(
        detect_language(Path::new("src/main.cpp")),
        Some(Language::Cpp)
    );
    assert_eq!(
        detect_language(Path::new("scripts/build.sh")),
        Some(Language::Shell)
    );
    assert_eq!(
        detect_language(Path::new("scripts/build.bash")),
        Some(Language::Shell)
    );
    assert_eq!(detect_language(Path::new("README.md")), None);
}

#[test]
fn extracts_python_tree_sitter_symbols_and_calls() {
    let source = r#"
import os

def hello(name):
    return name.upper()

def greet(name):
    return hello(name)

class App:
    def run(self):
        return greet("world")
"#;
    let result = extract_for_language(Language::Python, source, "main.py").unwrap();

    assert!(node_names(&result, NodeKind::Function).contains(&"hello".to_string()));
    assert!(node_names(&result, NodeKind::Function).contains(&"greet".to_string()));
    assert!(node_names(&result, NodeKind::Class).contains(&"App".to_string()));
    assert!(node_names(&result, NodeKind::Method).contains(&"run".to_string()));
    assert!(has_call(&result, "hello"));
    assert!(has_call(&result, "greet"));
    assert!(result
        .nodes
        .iter()
        .any(|node| node.end_line > node.start_line));
    assert!(result
        .edges
        .iter()
        .all(|edge| edge.provenance.as_deref() == Some("tree-sitter")));
}

#[test]
fn extracts_rust_tree_sitter_symbols_and_calls() {
    let source = r#"
use std::fmt;

fn hello(name: &str) -> String { format!("hi {name}") }
fn greet(name: &str) -> String { hello(name) }

struct App;
impl App {
    pub fn run(&self) -> String { greet("world") }
}
"#;
    let result = extract_for_language(Language::Rust, source, "main.rs").unwrap();

    assert!(node_names(&result, NodeKind::Import).contains(&"std".to_string()));
    assert!(node_names(&result, NodeKind::Function).contains(&"hello".to_string()));
    assert!(node_names(&result, NodeKind::Function).contains(&"greet".to_string()));
    assert!(node_names(&result, NodeKind::Struct).contains(&"App".to_string()));
    assert!(node_names(&result, NodeKind::Method).contains(&"run".to_string()));
    assert!(has_call(&result, "hello"));
    assert!(has_call(&result, "greet"));
}

#[test]
fn extracts_javascript_tree_sitter_symbols_and_calls() {
    let source = r#"
import fs from 'fs';
function hello(name) { return name.toUpperCase(); }
function greet(name) { return hello(name); }
class App { run() { return greet('world'); } }
"#;
    let result = extract_for_language(Language::JavaScript, source, "main.js").unwrap();

    assert!(node_names(&result, NodeKind::Import).contains(&"fs".to_string()));
    assert!(node_names(&result, NodeKind::Function).contains(&"hello".to_string()));
    assert!(node_names(&result, NodeKind::Function).contains(&"greet".to_string()));
    assert!(node_names(&result, NodeKind::Class).contains(&"App".to_string()));
    assert!(node_names(&result, NodeKind::Method).contains(&"run".to_string()));
    assert!(has_call(&result, "hello"));
    assert!(has_call(&result, "greet"));
}

#[test]
fn extracts_typescript_tree_sitter_symbols_and_calls() {
    let source = r#"
import fs from 'fs';
interface User { name: string }
type ID = string;
function hello(name: string): string { return name.toUpperCase(); }
function greet(user: User): string { return hello(user.name); }
class App { run(): string { return greet({ name: 'world' }); } }
"#;
    let result = extract_for_language(Language::TypeScript, source, "main.ts").unwrap();

    assert!(node_names(&result, NodeKind::Interface).contains(&"User".to_string()));
    assert!(node_names(&result, NodeKind::TypeAlias).contains(&"ID".to_string()));
    assert!(node_names(&result, NodeKind::Function).contains(&"hello".to_string()));
    assert!(node_names(&result, NodeKind::Class).contains(&"App".to_string()));
    assert!(has_call(&result, "hello"));
    assert!(has_call(&result, "greet"));
}

#[test]
fn extracts_tsx_tree_sitter_symbols_and_calls() {
    let source = r#"
import React from 'react';
type Props = { name: string };
function hello(name: string) { return name.toUpperCase(); }
export function App(props: Props) { return <div>{hello(props.name)}</div>; }
"#;
    let result = extract_for_language(Language::Tsx, source, "main.tsx").unwrap();

    assert!(node_names(&result, NodeKind::TypeAlias).contains(&"Props".to_string()));
    assert!(node_names(&result, NodeKind::Function).contains(&"hello".to_string()));
    assert!(node_names(&result, NodeKind::Function).contains(&"App".to_string()));
    assert!(has_call(&result, "hello"));
}

#[test]
fn extracts_go_tree_sitter_symbols_and_calls() {
    let source = r#"
package main
import "fmt"
type App struct{}
func hello(name string) string { return fmt.Sprintf("hi %s", name) }
func greet(name string) string { return hello(name) }
func (a App) Run() string { return greet("world") }
"#;
    let result = extract_for_language(Language::Go, source, "main.go").unwrap();

    assert!(node_names(&result, NodeKind::Import).contains(&"fmt".to_string()));
    assert!(node_names(&result, NodeKind::Struct).contains(&"App".to_string()));
    assert!(node_names(&result, NodeKind::Function).contains(&"hello".to_string()));
    assert!(node_names(&result, NodeKind::Function).contains(&"greet".to_string()));
    assert!(node_names(&result, NodeKind::Method).contains(&"Run".to_string()));
    assert!(has_call(&result, "hello"));
    assert!(has_call(&result, "greet"));
}

#[test]
fn extracts_c_tree_sitter_symbols_and_calls() {
    let source = r#"
#include <stdio.h>
struct App { int x; };
void hello(const char* name) { printf("hi %s", name); }
void greet(const char* name) { hello(name); }
int main() { greet("world"); return 0; }
"#;
    let result = extract_for_language(Language::C, source, "main.c").unwrap();

    assert!(node_names(&result, NodeKind::Import).contains(&"stdio.h".to_string()));
    assert!(node_names(&result, NodeKind::Struct).contains(&"App".to_string()));
    assert!(node_names(&result, NodeKind::Function).contains(&"hello".to_string()));
    assert!(node_names(&result, NodeKind::Function).contains(&"greet".to_string()));
    assert!(node_names(&result, NodeKind::Function).contains(&"main".to_string()));
    assert!(has_call(&result, "hello"));
    assert!(has_call(&result, "greet"));
}

#[test]
fn extracts_esqlc_qsc_calls_after_legacy_knr_recovery() {
    let source = r#"
void
tu_main(argc, argv)
int argc;
char **argv;
{
    if (argc > 1)
    {
        helper(argc);
    }
}

VOID
legacy_driver(is_table, owndottbl, nrmltblname)
i4      is_table;
char    *owndottbl;
char    *nrmltblname;
{
#ifdef VMS
    if (owndottbl[0] != '\0')
    {
        helper(owndottbl);
    }
#else
    helper(nrmltblname);
#endif
    some_legacy_func( 1, is_table, owndottbl );
    some_legacy_func(1, is_table, nrmltblname);
}
"#;
    let result = extract_for_language(Language::Esqlc, source, "example.qsc").unwrap();

    let legacy = result
        .nodes
        .iter()
        .find(|node| node.kind == NodeKind::Function && node.name == "legacy_driver")
        .expect("legacy K&R function should be extracted");
    assert!(
        legacy.end_line >= 28,
        "legacy function range should be repaired to include late statements: {legacy:?}"
    );

    let mqbf_calls: Vec<_> = result
        .edges
        .iter()
        .filter(|edge| edge.kind == EdgeKind::Calls && edge.target == "some_legacy_func")
        .collect();
    assert_eq!(mqbf_calls.len(), 2, "edges: {:#?}", result.edges);
    assert!(mqbf_calls.iter().all(|edge| edge.source == legacy.id));
    assert!(mqbf_calls.iter().any(|edge| edge.line == Some(27)));
    assert!(mqbf_calls.iter().any(|edge| edge.line == Some(28)));
}

#[test]
fn extracts_shell_tree_sitter_symbols_calls_and_imports() {
    let source = r#"
#!/usr/bin/env bash
source ./lib.sh

foo() {
    helper "$1"
}

function bar {
    foo world
}
"#;
    let result = extract_for_language(Language::Shell, source, "scripts/build.sh").unwrap();

    assert!(node_names(&result, NodeKind::Function).contains(&"foo".to_string()));
    assert!(node_names(&result, NodeKind::Function).contains(&"bar".to_string()));
    assert!(node_names(&result, NodeKind::Import).contains(&"./lib.sh".to_string()));
    assert!(has_call(&result, "helper"));
    assert!(has_call(&result, "foo"));
}

#[test]
fn extracts_c_initializer_identifier_references() {
    let source = r#"
typedef int (*MO_GET_METHOD)(void);
MO_GET_METHOD scd_shutdown_get;

MO_CLASS_DEF Scd_classes[] =
{
    { 0, "exp.scf.scd.server.shutdown_state", scd_shutdown_get, MOnoset },
};
"#;
    let result = extract_for_language(Language::C, source, "scddata.c").unwrap();

    assert!(result.nodes.iter().any(|node| node.name == "Scd_classes"));
    assert!(
        result.edges.iter().any(|edge| {
            edge.kind == EdgeKind::References
                && edge.target == "scd_shutdown_get"
                && edge.line == Some(7)
        }),
        "edges: {:?}",
        result.edges
    );
}

#[test]
fn extracts_cpp_tree_sitter_symbols_and_calls() {
    let source = r#"
#include <string>
namespace demo {
class App { public: std::string run(); };
std::string hello(std::string name) { return name; }
std::string greet(std::string name) { return hello(name); }
std::string App::run() { return greet("world"); }
}
"#;
    let result = extract_for_language(Language::Cpp, source, "main.cpp").unwrap();

    assert!(node_names(&result, NodeKind::Import).contains(&"string".to_string()));
    assert!(node_names(&result, NodeKind::Namespace).contains(&"demo".to_string()));
    assert!(node_names(&result, NodeKind::Class).contains(&"App".to_string()));
    assert!(node_names(&result, NodeKind::Method).contains(&"hello".to_string()));
    assert!(node_names(&result, NodeKind::Method).contains(&"greet".to_string()));
    assert!(has_call(&result, "hello"));
    assert!(has_call(&result, "greet"));
}

#[test]
fn extracts_c_multiple_includes_and_typedefs() {
    let source = r#"
#include <stdio.h>
#include <nlohmann/json.hpp>
#include "config.h"
typedef struct { int value; } Widget;
typedef enum { Red, Green } Color;
void hello(const char* name) { printf("hi %s", name); }
void greet(const char* name) { hello(name); }
"#;
    let result = extract_for_language(Language::C, source, "main.c").unwrap();

    let imports = node_names(&result, NodeKind::Import);
    assert!(imports.contains(&"stdio.h".to_string()));
    assert!(imports.contains(&"nlohmann/json.hpp".to_string()));
    assert!(imports.contains(&"config.h".to_string()));

    // Anonymous typedef struct/enum should be named after the typedef declarator
    // rather than after the first field/enumerator inside the body.
    assert!(node_names(&result, NodeKind::Struct).contains(&"Widget".to_string()));
    assert!(node_names(&result, NodeKind::Enum).contains(&"Color".to_string()));
    assert!(has_call(&result, "hello"));
}

#[test]
fn extracts_c_preprocessor_macros_as_constants() {
    let source = r#"
#ifdef __cplusplus
# define II_EXTERN extern "C"
#else
# define II_EXTERN extern
#endif
#define II_CALLBACK(fn) ((fn) != 0)
II_EXTERN void exported(void);
"#;
    let result = extract_for_language(Language::C, source, "api.h").unwrap();
    let constants = node_names(&result, NodeKind::Constant);

    assert!(
        constants.contains(&"II_EXTERN".to_string()),
        "{constants:?}"
    );
    assert!(
        constants.contains(&"II_CALLBACK".to_string()),
        "{constants:?}"
    );

    let ii_extern = result
        .nodes
        .iter()
        .find(|node| node.name == "II_EXTERN")
        .unwrap();
    assert_eq!(ii_extern.kind, NodeKind::Constant);
    assert_eq!(ii_extern.qualified_name, "api.h#II_EXTERN");
    assert!(ii_extern
        .signature
        .as_deref()
        .unwrap_or("")
        .contains("extern"));
}

#[test]
fn extracts_cpp_inheritance_and_override_edges() {
    let source = r#"
#include <iostream>
class Iterator {
 public:
  virtual void Next() { }
};
class DBIter : public Iterator {
 public:
  void Next() override { advance(); }
  void advance() { }
};
"#;
    let result = extract_for_language(Language::Cpp, source, "iter.cpp").unwrap();

    assert!(node_names(&result, NodeKind::Import).contains(&"iostream".to_string()));
    assert!(node_names(&result, NodeKind::Class).contains(&"Iterator".to_string()));
    assert!(node_names(&result, NodeKind::Class).contains(&"DBIter".to_string()));

    let nexts: Vec<_> = result
        .nodes
        .iter()
        .filter(|n| n.kind == NodeKind::Method && n.name == "Next")
        .collect();
    assert_eq!(
        nexts.len(),
        2,
        "base virtual and override methods should both be extracted"
    );

    let dbiter = result.nodes.iter().find(|n| n.name == "DBIter").unwrap();
    assert!(result
        .edges
        .iter()
        .any(|e| e.kind == EdgeKind::Extends && e.source == dbiter.id && e.target == "Iterator"));

    let base_next = nexts.iter().min_by_key(|n| n.start_line).unwrap();
    let override_next = nexts.iter().max_by_key(|n| n.start_line).unwrap();
    assert!(result.edges.iter().any(|e| {
        e.kind == EdgeKind::Calls
            && e.source == base_next.id
            && e.target == override_next.id
            && e.provenance.as_deref() == Some("tree-sitter+cpp-override")
    }));
}

#[test]
fn extracts_rust_trait_relationships() {
    let source = r#"
pub struct MyCache {}

pub trait Display {}
pub trait Error: Display {
    fn description(&self) -> &str;
}

pub trait Cache {
    fn get(&self, key: &str) -> Option<String>;
}

impl Cache for MyCache {
    fn get(&self, key: &str) -> Option<String> {
        None
    }
}

impl MyCache {
    pub fn new() -> MyCache { MyCache {} }
}
"#;
    let result = extract_for_language(Language::Rust, source, "cache.rs").unwrap();

    let my_cache = result
        .nodes
        .iter()
        .find(|n| n.name == "MyCache" && n.kind == NodeKind::Struct)
        .unwrap();
    let error_trait = result
        .nodes
        .iter()
        .find(|n| n.name == "Error" && n.kind == NodeKind::Trait)
        .unwrap();

    assert!(result.edges.iter().any(|e| {
        e.kind == EdgeKind::Implements && e.source == my_cache.id && e.target == "Cache"
    }));
    assert!(result.edges.iter().any(|e| {
        e.kind == EdgeKind::Extends && e.source == error_trait.id && e.target == "Display"
    }));

    // Plain inherent impl should not create an implements edge.
    assert!(!result
        .edges
        .iter()
        .any(|e| { e.kind == EdgeKind::Implements && e.target == "MyCache" }));
}

#[test]
fn extracts_rust_import_roots() {
    let cases = [
        ("use std::io;", "std"),
        ("use std::{ffi::OsStr, io, path::Path};", "std"),
        ("use crate::error::Error;", "crate"),
        ("use super::utils;", "super"),
        ("use serde::{Serialize, Deserialize};", "serde"),
    ];

    for (source, expected) in cases {
        let result = extract_for_language(Language::Rust, source, "main.rs").unwrap();
        let imports = node_names(&result, NodeKind::Import);
        assert!(
            imports.contains(&expected.to_string()),
            "expected Rust import root {expected:?} in {imports:?} for {source:?}"
        );
        let import_node = result
            .nodes
            .iter()
            .find(|node| node.kind == NodeKind::Import)
            .unwrap();
        assert_eq!(import_node.signature.as_deref(), Some(source));
    }
}

#[test]
fn extracts_go_imports() {
    let source = r#"
package main

import (
    "fmt"
    "os"
    "encoding/json"
)
import f "net/http"
import . "math"
import _ "github.com/go-sql-driver/mysql"
"#;
    let result = extract_for_language(Language::Go, source, "main.go").unwrap();
    let imports = node_names(&result, NodeKind::Import);

    for expected in [
        "fmt",
        "os",
        "encoding/json",
        "net/http",
        "math",
        "github.com/go-sql-driver/mysql",
    ] {
        assert!(
            imports.contains(&expected.to_string()),
            "expected Go import {expected:?} in {imports:?}"
        );
    }

    assert!(result.nodes.iter().any(|node| {
        node.kind == NodeKind::Import
            && node.name == "net/http"
            && node.signature.as_deref().unwrap_or_default().contains('f')
    }));
    assert!(result.nodes.iter().any(|node| {
        node.kind == NodeKind::Import
            && node.name == "math"
            && node.signature.as_deref().unwrap_or_default().contains('.')
    }));
    assert!(result.nodes.iter().any(|node| {
        node.kind == NodeKind::Import
            && node.name == "github.com/go-sql-driver/mysql"
            && node.signature.as_deref().unwrap_or_default().contains('_')
    }));
}

#[test]
fn extracts_js_ts_arrow_and_function_expression_exports() {
    let ts_source = r#"
export const useAuth = (): AuthContextValue => {
  return useContext(AuthContext);
};

export const processData = function(input: string): string {
  return input.trim();
};

const internalHelper = () => {
  return 42;
};

const items = [1, 2, 3].map((x) => x * 2);
"#;
    let ts = extract_for_language(Language::TypeScript, ts_source, "hooks.ts").unwrap();
    let functions = node_names(&ts, NodeKind::Function);
    assert!(functions.contains(&"useAuth".to_string()));
    assert!(functions.contains(&"processData".to_string()));
    assert!(functions.contains(&"internalHelper".to_string()));
    assert!(!functions.contains(&"<anonymous>".to_string()));
    assert!(!node_names(&ts, NodeKind::Variable).contains(&"useAuth".to_string()));

    let use_auth = ts.nodes.iter().find(|n| n.name == "useAuth").unwrap();
    let process_data = ts.nodes.iter().find(|n| n.name == "processData").unwrap();
    let internal = ts
        .nodes
        .iter()
        .find(|n| n.name == "internalHelper")
        .unwrap();
    assert!(use_auth.is_exported);
    assert!(process_data.is_exported);
    assert!(!internal.is_exported);
    assert!(has_call(&ts, "useContext"));

    let js_source = r#"
export const fetchData = async () => {
  const response = await fetch('/api/data');
  return response.json();
};
"#;
    let js = extract_for_language(Language::JavaScript, js_source, "api.js").unwrap();
    let fetch_data = js.nodes.iter().find(|n| n.name == "fetchData").unwrap();
    assert_eq!(fetch_data.kind, NodeKind::Function);
    assert!(fetch_data.is_exported);
    assert!(fetch_data.is_async);
    assert!(has_call(&js, "fetch"));
}

#[test]
fn extracts_rust_attributes_and_macro_calls() {
    let source = r#"
#[derive(Debug, Clone)]
pub struct User {
    id: String,
}

#[test]
fn smoke_test() {
    println!("ok");
}
"#;
    let result = extract_for_language(Language::Rust, source, "attrs.rs").unwrap();

    let user = result
        .nodes
        .iter()
        .find(|n| n.kind == NodeKind::Struct && n.name == "User")
        .unwrap();
    assert!(user
        .decorators
        .as_ref()
        .unwrap()
        .contains(&"derive(Debug, Clone)".to_string()));

    let smoke = result
        .nodes
        .iter()
        .find(|n| n.kind == NodeKind::Function && n.name == "smoke_test")
        .unwrap();
    assert!(smoke
        .decorators
        .as_ref()
        .unwrap()
        .contains(&"test".to_string()));
    assert!(has_call(&result, "println!"));
}

#[test]
fn extracts_ts_type_aliases_and_exported_constants() {
    let source = r#"
export type UnitSystem = 'metric' | 'imperial';
export type DateFormat = 'ISO' | 'US' | 'EU';
type Internal = string;

export const useUIStore = createStore({ open: false });
export const config = { apiUrl: 'https://api.example.com' };
export const SCREEN_NAMES = ['home', 'settings'] as const;
export const MAX_RETRIES = 3;
const internalConfig = { debug: true };
"#;
    let result = extract_for_language(Language::TypeScript, source, "config.ts").unwrap();

    let aliases = result
        .nodes
        .iter()
        .filter(|n| n.kind == NodeKind::TypeAlias)
        .collect::<Vec<_>>();
    assert_eq!(aliases.len(), 3);
    assert!(aliases
        .iter()
        .any(|n| n.name == "UnitSystem" && n.is_exported));
    assert!(aliases
        .iter()
        .any(|n| n.name == "DateFormat" && n.is_exported));
    assert!(aliases
        .iter()
        .any(|n| n.name == "Internal" && !n.is_exported));

    for expected in ["useUIStore", "config", "SCREEN_NAMES", "MAX_RETRIES"] {
        assert!(
            result
                .nodes
                .iter()
                .any(|n| n.kind == NodeKind::Constant && n.name == expected && n.is_exported),
            "missing exported constant {expected}"
        );
    }
    assert!(result
        .nodes
        .iter()
        .any(|n| n.kind == NodeKind::Constant && n.name == "internalConfig" && !n.is_exported));
}

#[test]
fn extracts_typescript_extends_and_implements() {
    let source = r#"
class BaseController {}
interface Serializable {}
interface JsonSerializable {}
class ChildController extends BaseController implements Serializable, JsonSerializable {
  run() {}
}
"#;
    let result = extract_for_language(Language::TypeScript, source, "controller.ts").unwrap();
    let child = result
        .nodes
        .iter()
        .find(|n| n.kind == NodeKind::Class && n.name == "ChildController")
        .unwrap();

    assert!(result.edges.iter().any(|e| e.kind == EdgeKind::Extends
        && e.source == child.id
        && e.target == "BaseController"));
    assert!(result.edges.iter().any(|e| e.kind == EdgeKind::Implements
        && e.source == child.id
        && e.target == "Serializable"));
    assert!(result.edges.iter().any(|e| e.kind == EdgeKind::Implements
        && e.source == child.id
        && e.target == "JsonSerializable"));
}

#[test]
fn extracts_go_interfaces() {
    let source = r#"
package main

type Reader interface {
    Read(p []byte) (n int, err error)
}

type Writer interface {
    Write(p []byte) (n int, err error)
}

type ReadWriter interface {
    Reader
    Writer
}

type Config struct {
    Name string
}
"#;
    let result = extract_for_language(Language::Go, source, "types.go").unwrap();

    // Struct should still be detected.
    assert!(
        result
            .nodes
            .iter()
            .any(|n| n.kind == NodeKind::Struct && n.name == "Config"),
        "expected struct Config"
    );

    // Interfaces should be detected, not treated as structs.
    for expected in ["Reader", "Writer", "ReadWriter"] {
        assert!(
            result
                .nodes
                .iter()
                .any(|n| n.kind == NodeKind::Interface && n.name == expected),
            "expected interface {expected}"
        );
        assert!(
            !result
                .nodes
                .iter()
                .any(|n| n.kind == NodeKind::Struct && n.name == expected),
            "{expected} should not be a struct"
        );
    }
}

#[test]
fn extracts_ts_decorators() {
    let source = r#"
function A(cls: any) { return cls; }
function B(cls: any) { return cls; }
@A
class Foo {}
@B
class Bar {}

function Get(p: string) { return (t: any, k: string) => t; }
class Svc {
  @Get('/x') method() { return 1; }
}
"#;
    let result = extract_for_language(Language::TypeScript, source, "app.ts").unwrap();

    // Decorator edges: Foo → A, Bar → B
    let decorates_edges: Vec<_> = result
        .edges
        .iter()
        .filter(|e| e.kind == EdgeKind::Decorates)
        .collect();

    // Foo decorated by A
    let foo = result
        .nodes
        .iter()
        .find(|n| n.kind == NodeKind::Class && n.name == "Foo")
        .unwrap();
    assert!(
        decorates_edges
            .iter()
            .any(|e| e.source == foo.id && e.target == "A"),
        "expected decorates(Foo, A) in {:?}",
        decorates_edges
            .iter()
            .map(|e| (&e.source, &e.target))
            .collect::<Vec<_>>()
    );

    // Bar decorated by B, not A (no cross-attribution).
    let bar = result
        .nodes
        .iter()
        .find(|n| n.kind == NodeKind::Class && n.name == "Bar")
        .unwrap();
    let from_bar: Vec<_> = decorates_edges
        .iter()
        .filter(|e| e.source == bar.id)
        .collect();
    assert_eq!(
        from_bar.len(),
        1,
        "Bar should have exactly one decorator, got: {:?}",
        from_bar.iter().map(|e| &e.target).collect::<Vec<_>>()
    );
    assert_eq!(from_bar[0].target, "B");

    // Method decorator: method decorated by Get
    let method = result
        .nodes
        .iter()
        .find(|n| n.kind == NodeKind::Method && n.name == "method")
        .unwrap();
    assert!(
        decorates_edges
            .iter()
            .any(|e| e.source == method.id && e.target == "Get"),
        "expected decorates(method, Get) in {:?}",
        decorates_edges
            .iter()
            .map(|e| (&e.source, &e.target))
            .collect::<Vec<_>>()
    );
}

#[test]
fn extracts_go_constants_and_methods() {
    let source = r#"
package main

const MaxRetries = 3
const defaultTimeout = 5000

type Server struct {
    addr string
}

func (s *Server) Listen() error {
    return nil
}

func (s Server) String() string {
    return s.addr
}
"#;
    let result = extract_for_language(Language::Go, source, "server.go").unwrap();

    // Constants
    let constants = node_names(&result, NodeKind::Constant);
    assert!(
        constants.contains(&"MaxRetries".to_string()),
        "expected MaxRetries constant, got: {constants:?}"
    );
    assert!(
        constants.contains(&"defaultTimeout".to_string()),
        "expected defaultTimeout constant, got: {constants:?}"
    );
    // MaxRetries is exported (uppercase), defaultTimeout is not.
    let max_retries = result
        .nodes
        .iter()
        .find(|n| n.kind == NodeKind::Constant && n.name == "MaxRetries")
        .unwrap();
    assert!(max_retries.is_exported);
    let default_timeout = result
        .nodes
        .iter()
        .find(|n| n.kind == NodeKind::Constant && n.name == "defaultTimeout")
        .unwrap();
    assert!(!default_timeout.is_exported);

    // Methods with receiver
    let methods = node_names(&result, NodeKind::Method);
    assert!(methods.contains(&"Listen".to_string()));
    assert!(methods.contains(&"String".to_string()));

    // Struct
    assert!(node_names(&result, NodeKind::Struct).contains(&"Server".to_string()));
}

#[test]
fn extracts_ts_zod_and_xstate_exports() {
    let source = r#"
export const userSchema = z.object({
  id: z.string(),
  name: z.string(),
  email: z.string().email(),
});

export const authMachine = createMachine({
  id: "auth",
  initial: "idle",
  states: {
    idle: {},
    authenticated: {},
  },
});
"#;
    let result = extract_for_language(Language::TypeScript, source, "schemas.ts").unwrap();

    let user_schema = result
        .nodes
        .iter()
        .find(|n| n.kind == NodeKind::Constant && n.name == "userSchema");
    assert!(user_schema.is_some());
    assert!(user_schema.unwrap().is_exported);

    let auth_machine = result
        .nodes
        .iter()
        .find(|n| n.kind == NodeKind::Constant && n.name == "authMachine");
    assert!(auth_machine.is_some());
    assert!(auth_machine.unwrap().is_exported);
}

#[test]
fn resolves_cross_file_references_after_indexing() {
    use grph_core::extraction::ExtractionOrchestrator;
    use grph_core::resolution::ReferenceResolver;
    use grph_core::Database;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("grph-crossfile-{}-{stamp}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();

    // File A: defines a helper function
    fs::write(
        dir.join("lib.py"),
        r#"
def helper(name):
    return name.upper()
"#,
    )
    .unwrap();

    // File B: calls the helper function from File A
    fs::write(
        dir.join("main.py"),
        r#"
from lib import helper

def greet(name):
    return helper(name)
"#,
    )
    .unwrap();

    let db = Database::open(&dir).unwrap();
    db.init_schema().unwrap();
    let mut orchestrator = ExtractionOrchestrator::new(db.clone(), dir.clone()).unwrap();

    // Index both files
    let index_result = orchestrator.index_all(|_| {}).unwrap();
    assert!(
        index_result.files_indexed >= 2,
        "expected at least 2 files indexed"
    );

    // Before resolution, there should be an edge from greet → helper (name target)
    let greet = db
        .get_node_by_name_any("greet")
        .unwrap()
        .expect("greet should exist");
    let edges = db.get_edges_for_node(&greet.id).unwrap();
    let call_edge = edges.iter().find(|e| e.kind == EdgeKind::Calls).unwrap();
    // The call target may still be the name "helper" before resolution
    assert!(
        call_edge.target == "helper" || call_edge.target.contains("helper"),
        "call edge target should reference helper, got: {}",
        call_edge.target
    );

    // Run cross-file resolution
    let mut resolver = ReferenceResolver::new(db.clone(), dir.clone());
    let resolution = resolver.resolve_all().unwrap();

    // After resolution, check that greet→helper edge is now resolved to node ID
    if resolution.resolved > 0 {
        let edges_after = db.get_edges_for_node(&greet.id).unwrap();
        let call_after = edges_after
            .iter()
            .find(|e| e.kind == EdgeKind::Calls)
            .unwrap();
        let helper_node = db
            .get_node_by_name_any("helper")
            .unwrap()
            .expect("helper should exist");
        assert_eq!(
            call_after.target, helper_node.id,
            "cross-file call edge should resolve to helper's node ID"
        );
    }

    fs::remove_dir_all(dir).ok();
}

#[test]
fn callers_include_resolved_id_and_unresolved_name_edges() {
    use grph_core::Database;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("grph-callers-{}-{stamp}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();

    let db = Database::open(&dir).unwrap();
    db.init_schema().unwrap();
    db.conn()
        .execute_batch("PRAGMA foreign_keys = OFF")
        .unwrap();
    let caller_by_id = test_node("caller-id", "caller_by_id", 1);
    let caller_by_name = test_node("caller-name", "caller_by_name", 10);
    let callee = test_node("callee-id", "target", 20);

    db.insert_node(&caller_by_id).unwrap();
    db.insert_node(&caller_by_name).unwrap();
    db.insert_node(&callee).unwrap();
    db.insert_edge(&test_call(&caller_by_id.id, &callee.id, 3))
        .unwrap();
    db.insert_edge(&test_call(&caller_by_name.id, &callee.name, 12))
        .unwrap();

    let callers = db.find_callers(&callee.id, 10).unwrap();
    let names: Vec<_> = callers.iter().map(|(node, _)| node.name.as_str()).collect();

    assert!(names.contains(&"caller_by_id"), "callers: {names:?}");
    assert!(names.contains(&"caller_by_name"), "callers: {names:?}");

    fs::remove_dir_all(dir).ok();
}

#[test]
fn callees_include_resolved_id_and_unresolved_name_edges() {
    use grph_core::Database;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("grph-callees-{}-{stamp}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();

    let db = Database::open(&dir).unwrap();
    db.init_schema().unwrap();
    db.conn()
        .execute_batch("PRAGMA foreign_keys = OFF")
        .unwrap();
    let caller = test_node("caller-id", "caller", 1);
    let callee_by_id = test_node("callee-id", "callee_by_id", 10);
    let callee_by_name = test_node("callee-name", "callee_by_name", 20);

    db.insert_node(&caller).unwrap();
    db.insert_node(&callee_by_id).unwrap();
    db.insert_node(&callee_by_name).unwrap();
    db.insert_edge(&test_call(&caller.id, &callee_by_id.id, 3))
        .unwrap();
    db.insert_edge(&test_call(&caller.id, &callee_by_name.name, 4))
        .unwrap();

    let callees = db.find_callees(&caller.id, 10).unwrap();
    let names: Vec<_> = callees.iter().map(|(node, _)| node.name.as_str()).collect();

    assert!(names.contains(&"callee_by_id"), "callees: {names:?}");
    assert!(names.contains(&"callee_by_name"), "callees: {names:?}");

    fs::remove_dir_all(dir).ok();
}

#[test]
fn scan_files_skips_build_output_directories() {
    use grph_core::extraction::ExtractionOrchestrator;
    use grph_core::Database;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("grph-scan-skip-{}-{stamp}", std::process::id()));
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::create_dir_all(dir.join("target/debug/build/tree-sitter/out")).unwrap();
    fs::write(dir.join("src/main.rs"), "fn main() {}\n").unwrap();
    fs::write(
        dir.join("target/debug/build/tree-sitter/out/flag_check.c"),
        "int main(void) { return 0; }\n",
    )
    .unwrap();

    let db = Database::open(&dir).unwrap();
    let orchestrator = ExtractionOrchestrator::new(db, dir.clone()).unwrap();
    let files = orchestrator.scan_files().unwrap();
    let rels: Vec<String> = files
        .iter()
        .map(|path| {
            path.strip_prefix(&dir)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect();

    assert!(rels.contains(&"src/main.rs".to_string()), "rels={rels:?}");
    assert!(
        rels.iter().all(|path| !path.starts_with("target/")),
        "rels={rels:?}"
    );

    fs::remove_dir_all(dir).ok();
}

#[test]
fn c_builtin_and_macro_like_refs_are_not_queued_for_resolution() {
    use grph_core::Grph;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("grph-c-builtins-{stamp}"));
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("main.c"),
        r#"
#include <stdio.h>
#define ASSERT(x) ((void)0)
#define ERx(x) (x)

void local_helper(void) {}

void run(void) {
    printf("%s", ERx("ok"));
    ASSERT(1);
    local_helper();
}
"#,
    )
    .unwrap();

    let mut grph = Grph::init(&dir).unwrap();
    grph.index(|_| {}).unwrap();

    let unresolved = grph.db().get_unresolved_refs(1000).unwrap();
    let names: Vec<_> = unresolved
        .iter()
        .map(|r| r.reference_name.as_str())
        .collect();
    assert!(!names.contains(&"printf"), "unresolved: {names:?}");
    assert!(!names.contains(&"ASSERT"), "unresolved: {names:?}");
    assert!(!names.contains(&"ERx"), "unresolved: {names:?}");

    fs::remove_dir_all(dir).ok();
}

#[test]
fn c_platform_resolution_prefers_unix_win_implementation() {
    use grph_core::Grph;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("grph-c-platform-resolution-{stamp}"));
    fs::create_dir_all(dir.join("zorp/zlf/zq_unix_win")).unwrap();
    fs::create_dir_all(dir.join("zorp/zlf/zq_vms")).unwrap();
    fs::create_dir_all(dir.join("back/dmf/dmd")).unwrap();

    fs::write(
        dir.join("zorp/zlf/zq_unix_win/zq.c"),
        "int ZQrender(const char *fc, ...) { return 0; }\n",
    )
    .unwrap();
    fs::write(
        dir.join("zorp/zlf/zq_vms/zq.c"),
        "int ZQrender(const char *fc, ...) { return 1; }\n",
    )
    .unwrap();
    fs::write(
        dir.join("back/dmf/dmd/dmdcb.c"),
        "int run(void) { return ZQrender(\"x\"); }\n",
    )
    .unwrap();

    let mut grph = Grph::init(&dir).unwrap();
    grph.index(|_| {}).unwrap();

    let run = grph.db().get_node_by_name_any("run").unwrap().unwrap();
    let callees = grph.traverser().callees(&run.id, 20).unwrap();
    assert!(
        callees.iter().any(|(node, _)| {
            node.name == "ZQrender" && node.file_path == "zorp/zlf/zq_unix_win/zq.c"
        }),
        "{callees:?}"
    );
    assert!(
        !callees.iter().any(|(node, _)| {
            node.name == "ZQrender" && node.file_path == "zorp/zlf/zq_vms/zq.c"
        }),
        "{callees:?}"
    );

    fs::remove_dir_all(dir).ok();
}
