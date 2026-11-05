use super::config::without_jsonc_syntax;
use super::*;
use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
    navigator: Navigator,
}

impl Fixture {
    fn new(files: &[(&str, &str)]) -> Self {
        let root = std::env::temp_dir().join(format!(
            "hive-navigation-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        for (path, text) in files {
            let path = root.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }
        let root = fs::canonicalize(root).unwrap();
        Self {
            root,
            navigator: Navigator::default(),
        }
    }

    /// Definition of the symbol that starts the last occurrence of `needle` in `file`, clicked
    /// in its middle; the rest of `needle` only disambiguates the occurrence.
    fn definition(&self, file: &str, needle: &str) -> Option<(String, u32, u32)> {
        let text = fs::read_to_string(self.root.join(file)).unwrap();
        let start = text.rfind(needle).expect("needle in text");
        let symbol = needle
            .find(|c: char| !(c.is_alphanumeric() || matches!(c, '_' | '$' | '#')))
            .unwrap_or(needle.len());
        assert!(symbol > 0, "needle must start with a symbol");
        let offset = start + symbol / 2;
        let definition = self
            .navigator
            .definition(&self.root, Path::new(file), &text, offset)
            .unwrap()?;
        assert_eq!(definition.origin, start..start + symbol);
        Some((
            definition.path.to_string_lossy().into_owned(),
            definition.line,
            definition.column,
        ))
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).unwrap();
    }
}

fn hit(path: &str, line: u32, column: u32) -> Option<(String, u32, u32)> {
    Some((path.to_owned(), line, column))
}

#[test]
fn subpath_import_through_barrel_reaches_the_declaration() {
    let f = Fixture::new(&[
        (
            "package.json",
            r##"{"type":"module","imports":{"#utils":"./source/utils/index.js","#lib/*":"./source/lib/*.js"}}"##,
        ),
        (
            "source/utils/index.js",
            "export { default as cacheHelper } from './cacheHelper.js';\nexport * from './errors.js';\nexport * from './format.js';\n",
        ),
        (
            "source/utils/cacheHelper.js",
            "// cache\nconst cacheHelper = {\n  get() {},\n};\nexport default cacheHelper;\n",
        ),
        ("source/utils/errors.js", "export const defaultError = 1;\n"),
        (
            "source/utils/format.js",
            "/** Формат */\nexport function formatLineJson(line) {\n  return line;\n}\n",
        ),
        (
            "source/lib/auth.js",
            "export const getAuthorizationUserId = () => 1;\n",
        ),
        (
            "source/routes/user.js",
            "import { defaultError, cacheHelper, formatLineJson } from '#utils';\nimport { getAuthorizationUserId } from '#lib/auth';\n\ncacheHelper.get(formatLineJson(defaultError), getAuthorizationUserId());\n",
        ),
    ]);
    let file = "source/routes/user.js";
    assert_eq!(
        f.definition(file, "cacheHelper"),
        hit("source/utils/cacheHelper.js", 1, 6)
    );
    assert_eq!(
        f.definition(file, "formatLineJson"),
        hit("source/utils/format.js", 1, 16)
    );
    assert_eq!(
        f.definition(file, "defaultError"),
        hit("source/utils/errors.js", 0, 13)
    );
    assert_eq!(
        f.definition(file, "getAuthorizationUserId"),
        hit("source/lib/auth.js", 0, 13)
    );
    assert_eq!(
        f.definition(file, "#utils"),
        hit("source/utils/index.js", 0, 0)
    );
}

#[test]
fn import_statement_names_are_clickable_too() {
    let f = Fixture::new(&[
        ("a.js", "import { helper as h } from './b';\nh();\n"),
        ("b.js", "\nexport function helper() {}\n"),
    ]);
    assert_eq!(f.definition("a.js", "helper"), hit("b.js", 1, 16));
    assert_eq!(f.definition("a.js", "h()"), hit("b.js", 1, 16));
}

#[test]
fn tsconfig_paths_with_extends_comments_and_js_extension() {
    let f = Fixture::new(&[
        (
            "tsconfig.base.json",
            "{\n  // shared\n  \"compilerOptions\": {\n    \"baseUrl\": \".\",\n    \"paths\": { \"@/*\": [\"src/*\"], },\n  },\n}\n",
        ),
        (
            "tsconfig.json",
            "{ \"extends\": \"./tsconfig.base\" /* local */ }",
        ),
        (
            "src/services/user.ts",
            "export class UserService {}\nexport type Id = string;\n",
        ),
        (
            "src/models/index.ts",
            "export interface User { id: string }\n",
        ),
        ("src/util.ts", "export const one = 1;\n"),
        (
            "src/app.ts",
            "import { UserService, type Id } from '@/services/user';\nimport type { User } from './models';\nimport { one } from './util.js';\nconst u: User = new UserService();\nconst i: Id = one;\n",
        ),
    ]);
    assert_eq!(
        f.definition("src/app.ts", "UserService"),
        hit("src/services/user.ts", 0, 13)
    );
    assert_eq!(
        f.definition("src/app.ts", "Id"),
        hit("src/services/user.ts", 1, 12)
    );
    assert_eq!(
        f.definition("src/app.ts", "User ="),
        hit("src/models/index.ts", 0, 17)
    );
    assert_eq!(f.definition("src/app.ts", "one"), hit("src/util.ts", 0, 13));
}

#[test]
fn commonjs_and_namespace_members() {
    let f = Fixture::new(&[
        (
            "lib/math.js",
            "function add(a, b) { return a + b; }\nexports.sub = (a, b) => a - b;\nmodule.exports.mul = mul;\nfunction mul() {}\n",
        ),
        (
            "lib/config.js",
            "const port = 1;\nmodule.exports = { port, host: 'x', load() {} };\n",
        ),
        (
            "lib/single.js",
            "class Client {}\nmodule.exports = Client;\n",
        ),
        (
            "main.js",
            "const math = require('./lib/math');\nconst { port, load: boot } = require('./lib/config');\nconst Client = require('./lib/single');\nimport * as m from './lib/math';\nimport C from './lib/single';\nmath.sub(port);\nm.mul();\nboot(Client, C);\n",
        ),
    ]);
    assert_eq!(f.definition("main.js", "sub"), hit("lib/math.js", 1, 8));
    assert_eq!(f.definition("main.js", "mul"), hit("lib/math.js", 3, 9));
    assert_eq!(f.definition("main.js", "port"), hit("lib/config.js", 0, 6));
    assert_eq!(f.definition("main.js", "boot"), hit("lib/config.js", 1, 36));
    assert_eq!(f.definition("main.js", "math."), hit("lib/math.js", 0, 0));
    assert_eq!(f.definition("main.js", "C)"), hit("lib/single.js", 0, 6));
}

#[test]
fn node_modules_package_entry_and_exports() {
    let f = Fixture::new(&[
        (
            "node_modules/plain/package.json",
            r#"{"main":"lib/main.js","types":"index.d.ts"}"#,
        ),
        (
            "node_modules/plain/index.d.ts",
            "export declare function run(): void;\n",
        ),
        (
            "node_modules/@scope/kit/package.json",
            r#"{"exports":{".":{"import":"./esm/index.js"},"./tools/*":"./esm/tools/*.js"}}"#,
        ),
        (
            "node_modules/@scope/kit/esm/index.js",
            "export const kit = 1;\n",
        ),
        (
            "node_modules/@scope/kit/esm/tools/saw.js",
            "export function saw() {}\n",
        ),
        (
            "app/index.js",
            "import { run } from 'plain';\nimport { kit } from '@scope/kit';\nimport { saw } from '@scope/kit/tools/saw';\nrun(kit, saw);\n",
        ),
    ]);
    assert_eq!(
        f.definition("app/index.js", "run"),
        hit("node_modules/plain/index.d.ts", 0, 24)
    );
    assert_eq!(
        f.definition("app/index.js", "kit"),
        hit("node_modules/@scope/kit/esm/index.js", 0, 13)
    );
    assert_eq!(
        f.definition("app/index.js", "saw"),
        hit("node_modules/@scope/kit/esm/tools/saw.js", 0, 16)
    );
}

#[test]
fn unresolved_and_unsafe_imports_give_nothing_or_the_module_top() {
    let f = Fixture::new(&[
        ("package.json", "{ broken"),
        ("a.js", "export * from './b.js';\nexport * from './a.js';\n"),
        ("b.js", "export const b = 1;\n"),
        (
            "src/main.js",
            "import fs from 'node:fs';\nimport { x } from 'missing';\nimport { y } from '../../outside';\nimport { z } from '#nope';\nimport { b, gone } from '../a.js';\nconst local = 1;\nfs(x, y, z, b, gone, local);\n",
        ),
    ]);
    let file = "src/main.js";
    for needle in ["fs(", "x,", "y,", "z,", "local)"] {
        assert_eq!(f.definition(file, needle), None, "{needle}");
    }
    assert_eq!(f.definition(file, "b,"), hit("b.js", 0, 13));
    assert_eq!(
        f.definition(file, "gone"),
        hit("a.js", 0, 0),
        "cycle ends at the module"
    );
}

#[test]
fn unsupported_files_are_ignored() {
    let f = Fixture::new(&[("notes.md", "import { a } from './a';\n")]);
    assert!(!is_supported(Path::new("notes.md")));
    assert!(is_supported(Path::new("App.TSX")));
    assert_eq!(f.definition("notes.md", "a"), None);
}

#[test]
fn jsonc_comments_and_trailing_commas_outside_strings() {
    let text = "{\"a\": \"//not\", /* c */ \"b\": [1, 2,], // end\n \"c\": \"x,]\",}";
    let value: serde_json::Value = serde_json::from_str(&without_jsonc_syntax(text)).unwrap();
    assert_eq!(value["a"], "//not");
    assert_eq!(value["b"], serde_json::json!([1, 2]));
    assert_eq!(value["c"], "x,]");
}

#[test]
fn line_column_counts_characters_not_bytes() {
    assert_eq!(line_column("а\nбв x", 8), (1, 3));
    assert_eq!(line_column("abc", 99), (0, 3));
}
