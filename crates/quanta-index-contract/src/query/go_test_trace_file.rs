//! Explicit Go test failure file-search selector over two pinned hybrid lanes.

use core::fmt;
use std::collections::BTreeSet;

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, MapAccess, Visitor},
    ser::SerializeStruct,
};

use crate::{
    ExactRepoRelativePathV1, GoTestTraceFrameClassV1, HybridQueryRequest, RepoId,
    validate_public_top_k,
};

/// Canonical source-frame hints parsed from the raw, public failure excerpt.
/// Matching text is query-bound; it does not prove an actual call-stack frame.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GoTestTraceSourceFramesV1 {
    paths: BTreeSet<ExactRepoRelativePathV1>,
}

impl GoTestTraceSourceFramesV1 {
    /// Parse relative `*.go:<line>` tokens as ranking hints, not frame authority.
    #[must_use]
    pub fn from_failure_excerpt(text: &str) -> Self {
        let mut paths = BTreeSet::new();
        for token in text.split_whitespace() {
            let token = token.trim_matches(|ch: char| {
                matches!(ch, '(' | ')' | '[' | ']' | ',' | '"' | '\'' | '`')
            });
            let Some((raw_path, line_and_rest)) = token.split_once(".go:") else {
                continue;
            };
            if !line_and_rest.starts_with(|ch: char| ch.is_ascii_digit()) {
                continue;
            }
            let raw_path = raw_path.strip_prefix("./").unwrap_or(raw_path);
            let path = format!("{raw_path}.go");
            if path.ends_with("_test.go") {
                continue;
            }
            if let Ok(path) = ExactRepoRelativePathV1::new(path) {
                let _inserted = paths.insert(path);
            }
        }
        Self { paths }
    }

    /// Select the first frame of an exact, versioned Go module after the last
    /// panic marker. Frames from a recovered stack before that marker, Go
    /// runtime, other modules, and tests cannot become this module's hint.
    /// The caller must separately prove that the module version belongs to
    /// the selected source revision; this parser only binds the raw text.
    #[must_use]
    pub fn from_module_cache_panic(text: &str, module: &GoTestTraceModuleContextV1) -> Self {
        let paths = Self::module_cache_panic_frame(text, module)
            .into_iter()
            .map(|frame| frame.path)
            .collect();
        Self { paths }
    }

    /// Parse source coordinates from the selected module-cache frame. An
    /// indented coordinate can be used as a path hint even when no declaration
    /// name is present. Source-authoritative callers must verify both against
    /// the pinned source bytes.
    #[must_use]
    pub fn module_cache_panic_frame(
        text: &str,
        module: &GoTestTraceModuleContextV1,
    ) -> Option<GoTestTraceModuleCacheFrameV1> {
        module.validate_v1().ok()?;
        let last_panic_line = text
            .lines()
            .enumerate()
            .filter_map(|(index, line)| {
                let line = line.trim_start();
                (line.starts_with("panic(") || line.starts_with("panic:")).then_some(index)
            })
            .last()?;
        let prefix = format!("{}@{}/", module.module_path, module.module_version);
        let mut opened_goroutine = false;
        let mut saw_stack_frame = false;
        let mut previous_line = None;
        for line in text.lines().skip(last_panic_line + 1) {
            let trimmed = line.trim_start();
            let preceding = previous_line.replace(trimmed);
            if let Some(header) = trimmed.strip_prefix("goroutine ") {
                if opened_goroutine || saw_stack_frame || !valid_goroutine_header(header) {
                    break;
                }
                opened_goroutine = true;
                continue;
            }
            if trimmed.starts_with("created by ")
                || ((opened_goroutine || saw_stack_frame) && trimmed.is_empty())
            {
                break;
            }
            if !trimmed.starts_with('/') {
                continue;
            }
            saw_stack_frame = true;
            let Some((_, cache_path)) = trimmed.split_once("/pkg/mod/") else {
                continue;
            };
            let Some(path_and_line) = cache_path.strip_prefix(&prefix) else {
                continue;
            };
            let Some((raw_path, line_tail)) = path_and_line.split_once(".go:") else {
                continue;
            };
            let Some(line_number) = line_tail.split_ascii_whitespace().next() else {
                continue;
            };
            if !line_number.bytes().all(|byte| byte.is_ascii_digit()) {
                continue;
            }
            let Ok(line_number) = line_number.parse::<std::num::NonZeroU32>() else {
                continue;
            };
            let symbol = preceding
                .and_then(|symbol| module_stack_symbol(symbol, &module.module_path, raw_path));
            // Go emits an indented coordinate after the function. If the
            // transcript lost indentation, require the adjacent declaration.
            if !(line.starts_with(' ') || line.starts_with('\t')) && symbol.is_none() {
                continue;
            }
            let path = format!("{raw_path}.go");
            if path.ends_with("_test.go") {
                return None;
            }
            return Some(GoTestTraceModuleCacheFrameV1 {
                path: ExactRepoRelativePathV1::new(path).ok()?,
                line: line_number.get(),
                symbol,
            });
        }
        None
    }

    #[must_use]
    pub fn frame_class(&self) -> GoTestTraceFrameClassV1 {
        match self.paths.len() {
            0 => GoTestTraceFrameClassV1::Unresolved,
            1 => GoTestTraceFrameClassV1::Exact,
            _ => GoTestTraceFrameClassV1::Ambiguous,
        }
    }

    #[must_use]
    pub fn paths(&self) -> &BTreeSet<ExactRepoRelativePathV1> {
        &self.paths
    }

    #[must_use]
    pub fn contains_path(&self, path: &str) -> bool {
        self.paths.iter().any(|frame| frame.as_str() == path)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GoTestTraceModuleCacheFrameV1 {
    pub path: ExactRepoRelativePathV1,
    pub line: u32,
    pub symbol: Option<String>,
}

fn valid_goroutine_header(header: &str) -> bool {
    let Some((details, status)) = header
        .strip_suffix(':')
        .and_then(|line| line.split_once(" ["))
    else {
        return false;
    };
    let Some(id) = details.split_ascii_whitespace().next() else {
        return false;
    };
    if !id.bytes().all(|byte| byte.is_ascii_digit()) || id.parse::<std::num::NonZeroU64>().is_err()
    {
        return false;
    }
    status.split_once(']').is_some_and(|(status, trailing)| {
        !status.is_empty()
            && (trailing.is_empty() || (trailing.starts_with(" {") && trailing.ends_with('}')))
    })
}

fn module_stack_symbol(line: &str, module_path: &str, source_path: &str) -> Option<String> {
    let rest = line.strip_prefix(module_path)?;
    let package_suffix = match source_path.rsplit_once('/') {
        Some((parent, _)) => rest.strip_prefix('/')?.strip_prefix(parent)?,
        None => rest,
    };
    let function = package_suffix.strip_prefix('.')?;
    let (symbol, arguments) = function.rsplit_once('(')?;
    if !arguments.ends_with(')') {
        return None;
    }
    let identifier = |value: &str| {
        let mut bytes = value.bytes();
        bytes
            .next()
            .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
            && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    };
    if let Some(receiver_and_name) = symbol.strip_prefix('(') {
        let (receiver, name) = receiver_and_name.split_once(").")?;
        let receiver = receiver.strip_prefix('*').unwrap_or(receiver);
        return (identifier(receiver) && identifier(name)).then(|| format!("{receiver}.{name}"));
    }
    identifier(symbol).then(|| symbol.to_owned())
}

/// Caller-asserted Go module-cache identity. The benchmark binds this exact
/// module and version to a frozen source commit before sending the request.
/// It remains a ranking hint, not server-side source provenance authority.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoTestTraceModuleContextV1 {
    pub module_path: String,
    pub module_version: String,
}

impl GoTestTraceModuleContextV1 {
    pub fn validate_v1(&self) -> Result<(), String> {
        let module = self.module_path.as_str();
        if module.len() > 256
            || module.split('/').count() < 2
            || module.split('/').any(|part| {
                part.is_empty()
                    || part == "."
                    || part == ".."
                    || !part.bytes().all(|ch| {
                        ch.is_ascii_lowercase()
                            || ch.is_ascii_digit()
                            || matches!(ch, b'.' | b'-' | b'_' | b'!')
                    })
            })
        {
            return Err("Go trace module path is invalid".to_string());
        }
        let version = self.module_version.as_str();
        if version.len() > 128
            || !version.starts_with('v')
            || !version[1..].bytes().any(|ch| ch.is_ascii_digit())
            || !version
                .bytes()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, b'.' | b'-' | b'+'))
        {
            return Err("Go trace module version is invalid".to_string());
        }
        Ok(())
    }
}

/// Maximum admitted public failure excerpt, independently of the dense input.
pub const GO_TEST_TRACE_FAILURE_MAX_BYTES_V1: usize = 16 * 1024;

/// Explicit Go test failure profile for investigating production faults.
/// Production `.go` files rank before `_test.go` files and docs. Parsed frames
/// from raw `failure_excerpt` are query-bound ranking hints, not call-stack
/// authority. `failure_source_repo_id` names the owner of a relative frame
/// when known; without it no file receives a frame hint. `query.top_k` bounds
/// each chunk lane; `top_k` bounds files.
#[derive(Clone, Debug, PartialEq)]
pub struct GoTestTraceFileQueryRequestV1 {
    pub query: HybridQueryRequest,
    pub failure_excerpt: String,
    pub failure_source_repo_id: Option<RepoId>,
    pub failure_module_context: Option<GoTestTraceModuleContextV1>,
    pub top_k: u32,
}

const FIELDS: &[&str] = &[
    "query",
    "failure_excerpt",
    "failure_source_repo_id",
    "failure_module_context",
    "top_k",
];

impl GoTestTraceFileQueryRequestV1 {
    pub fn validate_v1(&self) -> Result<(), String> {
        let chunk_top_k = validate_public_top_k(self.query.top_k)
            .map_err(|error| format!("{}: {error}", error.code()))?;
        let _lexical_top_k = validate_public_top_k(self.query.text_query.top_k)
            .map_err(|error| format!("{}: {error}", error.code()))?;
        let file_top_k = validate_public_top_k(self.top_k)
            .map_err(|error| format!("{}: {error}", error.code()))?;
        if file_top_k > chunk_top_k {
            return Err("Go trace file top_k exceeds the per-lane chunk cap".to_string());
        }
        if self.failure_excerpt.trim().is_empty()
            || self.failure_excerpt.contains('\0')
            || self.failure_excerpt.len() > GO_TEST_TRACE_FAILURE_MAX_BYTES_V1
        {
            return Err("Go trace failure_excerpt is empty or exceeds 16 KiB".to_string());
        }
        if let Some(module) = &self.failure_module_context {
            if self.failure_source_repo_id.is_none() || self.query.generation.is_none() {
                return Err(
                    "Go trace module context requires a source owner and generation pin"
                        .to_string(),
                );
            }
            module.validate_v1()?;
        }
        if self.query.semantic_query_text.trim().is_empty()
            || self.query.semantic_query_text.contains('\0')
            || self.query.semantic_query_text.len() > GO_TEST_TRACE_FAILURE_MAX_BYTES_V1
        {
            return Err("Go trace semantic query text is empty or exceeds 16 KiB".to_string());
        }
        if self.query.text_query.query_text.trim().is_empty()
            || self.query.text_query.query_text.contains('\0')
            || self.query.text_query.query_text.len() > GO_TEST_TRACE_FAILURE_MAX_BYTES_V1
        {
            return Err("Go trace lexical query text is empty or exceeds 16 KiB".to_string());
        }
        Ok(())
    }
}

impl Serialize for GoTestTraceFileQueryRequestV1 {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.validate_v1().map_err(serde::ser::Error::custom)?;
        let mut state = serializer.serialize_struct(
            "GoTestTraceFileQueryRequestV1",
            if self.failure_module_context.is_some() {
                5
            } else {
                4
            },
        )?;
        state.serialize_field("query", &self.query)?;
        state.serialize_field("failure_excerpt", &self.failure_excerpt)?;
        state.serialize_field("failure_source_repo_id", &self.failure_source_repo_id)?;
        if let Some(module) = &self.failure_module_context {
            state.serialize_field("failure_module_context", module)?;
        }
        state.serialize_field("top_k", &self.top_k)?;
        state.end()
    }
}

struct RequestVisitor;

impl<'de> Visitor<'de> for RequestVisitor {
    type Value = GoTestTraceFileQueryRequestV1;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a GoTestTraceFileQueryRequestV1 map")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut query = None;
        let mut failure_excerpt = None;
        let mut failure_source_repo_id: Option<Option<RepoId>> = None;
        let mut failure_module_context: Option<Option<GoTestTraceModuleContextV1>> = None;
        let mut top_k = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "query" => {
                    if query.is_some() {
                        return Err(de::Error::duplicate_field("query"));
                    }
                    query = Some(map.next_value()?);
                }
                "failure_excerpt" => {
                    if failure_excerpt.is_some() {
                        return Err(de::Error::duplicate_field("failure_excerpt"));
                    }
                    failure_excerpt = Some(map.next_value()?);
                }
                "failure_source_repo_id" => {
                    if failure_source_repo_id.is_some() {
                        return Err(de::Error::duplicate_field("failure_source_repo_id"));
                    }
                    failure_source_repo_id = Some(map.next_value()?);
                }
                "failure_module_context" => {
                    if failure_module_context.is_some() {
                        return Err(de::Error::duplicate_field("failure_module_context"));
                    }
                    failure_module_context = Some(map.next_value()?);
                }
                "top_k" => {
                    if top_k.is_some() {
                        return Err(de::Error::duplicate_field("top_k"));
                    }
                    top_k = Some(map.next_value()?);
                }
                other => return Err(de::Error::unknown_field(other, FIELDS)),
            }
        }
        let request = GoTestTraceFileQueryRequestV1 {
            query: query.ok_or_else(|| de::Error::missing_field("query"))?,
            failure_excerpt: failure_excerpt
                .ok_or_else(|| de::Error::missing_field("failure_excerpt"))?,
            failure_source_repo_id: failure_source_repo_id
                .ok_or_else(|| de::Error::missing_field("failure_source_repo_id"))?,
            failure_module_context: failure_module_context.unwrap_or(None),
            top_k: top_k.ok_or_else(|| de::Error::missing_field("top_k"))?,
        };
        request.validate_v1().map_err(de::Error::custom)?;
        Ok(request)
    }
}

impl<'de> Deserialize<'de> for GoTestTraceFileQueryRequestV1 {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_struct("GoTestTraceFileQueryRequestV1", FIELDS, RequestVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{QueryConstraintSetV1, TextQueryRequest, TextQuerySyntax};

    fn request() -> GoTestTraceFileQueryRequestV1 {
        GoTestTraceFileQueryRequestV1 {
            query: HybridQueryRequest {
                text_query: TextQueryRequest {
                    syntax: TextQuerySyntax::Native,
                    query_text: "case:no panic".to_string(),
                    constraints: QueryConstraintSetV1::unconstrained(),
                    generation: None,
                    generation_selector: None,
                    top_k: 100,
                    cursor: None,
                },
                semantic_query_text: "panic at binding.go:42".to_string(),
                generation: None,
                generation_selector: None,
                top_k: 100,
            },
            failure_excerpt: "$ go test ./...\npanic at binding.go:42".to_string(),
            failure_source_repo_id: None,
            failure_module_context: None,
            top_k: 20,
        }
    }

    #[test]
    fn request_roundtrips_and_refuses_unbounded_or_malformed_input() {
        let valid = request();
        let encoded = serde_json::to_string(&valid).expect("request encode");
        assert!(!encoded.contains("failure_module_context"));
        assert_eq!(
            serde_json::from_str::<GoTestTraceFileQueryRequestV1>(&encoded)
                .expect("request decode"),
            valid
        );
        assert!(encoded.contains("\"failure_source_repo_id\":null"));
        let missing_owner = encoded.replacen("\"failure_source_repo_id\":null,", "", 1);
        assert!(serde_json::from_str::<GoTestTraceFileQueryRequestV1>(&missing_owner).is_err());
        let duplicate_owner = encoded.replacen(
            "\"failure_source_repo_id\":null",
            "\"failure_source_repo_id\":null,\"failure_source_repo_id\":null",
            1,
        );
        assert!(serde_json::from_str::<GoTestTraceFileQueryRequestV1>(&duplicate_owner).is_err());
        let mut with_owner = valid.clone();
        with_owner.failure_source_repo_id = Some(RepoId::new("source-repo").expect("owner"));
        let with_owner_wire = serde_json::to_string(&with_owner).expect("owner encode");
        assert_eq!(
            serde_json::from_str::<GoTestTraceFileQueryRequestV1>(&with_owner_wire)
                .expect("owner decode"),
            with_owner
        );
        let module = GoTestTraceModuleContextV1 {
            module_path: "github.com/gofiber/fiber/v3".into(),
            module_version: "v3.0.0-rc.3".into(),
        };
        let mut module_request = with_owner.clone();
        module_request.failure_module_context = Some(module.clone());
        assert!(module_request.validate_v1().is_err());
        module_request.query.generation = Some(crate::GenerationPin::new(
            RepoId::new("source-repo").expect("repo"),
            crate::RevisionId::new("revision").expect("revision"),
            crate::ManifestGeneration::new(1),
        ));
        assert!(module_request.validate_v1().is_ok());
        let module_wire = serde_json::to_string(&module_request).expect("module request encode");
        assert!(module_wire.contains("failure_module_context"));
        assert_eq!(
            serde_json::from_str::<GoTestTraceFileQueryRequestV1>(&module_wire)
                .expect("module request decode"),
            module_request
        );
        let duplicate_module = module_wire.replacen(
            "\"failure_module_context\":",
            "\"failure_module_context\":null,\"failure_module_context\":",
            1,
        );
        assert!(serde_json::from_str::<GoTestTraceFileQueryRequestV1>(&duplicate_module).is_err());
        let mut invalid_module = module_request.clone();
        invalid_module.failure_module_context = Some(GoTestTraceModuleContextV1 {
            module_path: "github.com/../fiber".into(),
            ..module
        });
        assert!(invalid_module.validate_v1().is_err());
        module_request.failure_source_repo_id = None;
        assert!(module_request.validate_v1().is_err());
        let mut bad = valid.clone();
        bad.top_k = 101;
        assert!(bad.validate_v1().is_err());
        bad.top_k = 20;
        bad.failure_excerpt = "x".repeat(GO_TEST_TRACE_FAILURE_MAX_BYTES_V1 + 1);
        assert!(bad.validate_v1().is_err());
        bad.failure_excerpt = "\0".to_string();
        assert!(bad.validate_v1().is_err());
        bad = valid.clone();
        bad.query.text_query.top_k = 0;
        assert!(bad.validate_v1().is_err());
        let duplicate = encoded.replacen("\"top_k\":20", "\"top_k\":20,\"top_k\":20", 1);
        assert!(serde_json::from_str::<GoTestTraceFileQueryRequestV1>(&duplicate).is_err());
        let unknown = encoded.replacen(
            "\"failure_excerpt\":",
            "\"unknown\":1,\"failure_excerpt\":",
            1,
        );
        assert!(serde_json::from_str::<GoTestTraceFileQueryRequestV1>(&unknown).is_err());
    }

    #[test]
    fn module_cache_panic_uses_only_the_first_goroutine_stack() {
        let module = GoTestTraceModuleContextV1 {
            module_path: "github.com/example/producer/v2".into(),
            module_version: "v2.0.4".into(),
        };
        let frame_lines = concat!(
            "github.com/example/producer/v2/internal/worker.(*Worker).Run(0x0)\r\n",
            "        /go/pkg/mod/github.com/example/producer/v2@v2.0.4/internal/worker/worker.go:21 +0x13\r\n",
        );
        let header = "panic: runtime error\r\n[signal SIGSEGV]\r\n\r\n";
        let trace = format!("{header}goroutine 94 [running]:\r\n{frame_lines}");
        let result = GoTestTraceSourceFramesV1::module_cache_panic_frame(&trace, &module)
            .expect("first goroutine source coordinates");
        assert_eq!(result.path.as_str(), "internal/worker/worker.go");
        assert_eq!(result.line, 21);
        assert_eq!(result.symbol.as_deref(), Some("Worker.Run"));

        for foreign_prefix in [
            "goroutine 94 [running]:\r\ngoroutine 95 [running]:\r\n",
            "goroutine 94 [running]:\r\n\r\n",
            "goroutine bad [running]:\r\n",
            "goroutine 94 [running]\r\n",
        ] {
            let trace = format!("{header}{foreign_prefix}{frame_lines}");
            assert!(GoTestTraceSourceFramesV1::module_cache_panic_frame(&trace, &module).is_none());
        }
    }

    #[test]
    fn unindented_module_frame_requires_adjacent_exact_package_and_positive_line() {
        let module = GoTestTraceModuleContextV1 {
            module_path: "github.com/example/producer/v2".into(),
            module_version: "v2.0.4".into(),
        };
        let function = "github.com/example/producer/v2/internal/worker.(*Worker).Run(0x0)";
        let coordinate =
            "/go/pkg/mod/github.com/example/producer/v2@v2.0.4/internal/worker/worker.go:21 +0x13";
        let bind = |symbol: &str, location: &str| {
            GoTestTraceSourceFramesV1::from_module_cache_panic(
                &format!("panic({{0x1}})\r\n{symbol}\r\n{location}\r\n"),
                &module,
            )
        };
        assert!(bind(function, coordinate).contains_path("internal/worker/worker.go"));
        let frame = GoTestTraceSourceFramesV1::module_cache_panic_frame(
            &format!("panic: failure\n{function}\n{coordinate}\n"),
            &module,
        )
        .expect("module frame coordinates");
        assert_eq!(frame.path.as_str(), "internal/worker/worker.go");
        assert_eq!(frame.line, 21);
        assert_eq!(frame.symbol.as_deref(), Some("Worker.Run"));
        for symbol in [
            "",
            "github.com/example/producer/v2/internal/worker_extra.Run(0x0)",
            "github.com/example/producer/v2/internal/worker.other.Run(0x0)",
            "github.com/example/producer/v2/internal/worker.Run",
            "created by github.com/example/producer/v2/internal/worker.Run in goroutine 1",
        ] {
            assert_eq!(
                bind(symbol, coordinate).frame_class(),
                GoTestTraceFrameClassV1::Unresolved
            );
        }
        for line_number in ["0", "21junk", "4294967296"] {
            let invalid = coordinate.replace(":21 ", &format!(":{line_number} "));
            assert_eq!(
                bind(function, &invalid).frame_class(),
                GoTestTraceFrameClassV1::Unresolved
            );
        }
    }

    #[test]
    fn module_cache_panic_selects_first_own_production_frame_after_panic() {
        let module = GoTestTraceModuleContextV1 {
            module_path: "github.com/gofiber/fiber/v3".into(),
            module_version: "v3.0.0-rc.3".into(),
        };
        let excerpt = concat!(
            "panic: runtime error\n",
            "\t/Users/reporter/go/pkg/mod/github.com/gofiber/fiber/v3@v3.0.0-rc.3/middleware/recover/recover.go:12 +0x24\n",
            "panic({0x1})\n",
            "\t/Users/reporter/go/pkg/mod/github.com/valyala/fasthttp@v1.68.0/header.go:1163\n",
            "\t/Users/reporter/go/pkg/mod/github.com/gofiber/fiber/v3@v3.0.0-rc.3/middleware/encryptcookie/encryptcookie.go:23 +0x83c\n",
            "\t/Users/reporter/go/pkg/mod/github.com/gofiber/fiber/v3@v3.0.0-rc.3/ctx.go:235 +0x74",
        );
        let frames = GoTestTraceSourceFramesV1::from_module_cache_panic(excerpt, &module);
        assert_eq!(frames.frame_class(), GoTestTraceFrameClassV1::Exact);
        assert!(
            frames.contains_path("middleware/encryptcookie/encryptcookie.go"),
            "{:?}",
            frames.paths()
        );
        assert!(!frames.contains_path("middleware/recover/recover.go"));
        assert!(!frames.contains_path("ctx.go"));
        let wrong_version = GoTestTraceModuleContextV1 {
            module_version: "v3.0.0-rc.2".into(),
            ..module.clone()
        };
        assert_eq!(
            GoTestTraceSourceFramesV1::from_module_cache_panic(excerpt, &wrong_version)
                .frame_class(),
            GoTestTraceFrameClassV1::Unresolved
        );
        for raw in [
            "panic: failure\n\t/tmp/middleware/encryptcookie/encryptcookie.go:23",
            "panic: failure\n\t/tmp/pkg/mod/github.com/gofiber/fiber/v3@v3.0.0-rc.3/middleware/encryptcookie/encryptcookie_test.go:23",
            "panic: failure\n\t/tmp/pkg/mod/github.com/gofiber/fiber/v3@v3.0.0-rc.3/middleware/encryptcookie/encryptcookie_test.go:23\n\t/tmp/pkg/mod/github.com/gofiber/fiber/v3@v3.0.0-rc.3/middleware/encryptcookie/encryptcookie.go:23",
            "panic: failure\n\t/tmp/pkg/mod/github.com/gofiber/fiber/v3@v3.0.0-rc.3/../encryptcookie.go:23",
            "panic: failure\n\t/tmp/pkg/mod/github.com/other/module@v1.0.0/handler.go:2\n\ngoroutine 2 [running]\n\t/tmp/pkg/mod/github.com/gofiber/fiber/v3@v3.0.0-rc.3/middleware/encryptcookie/encryptcookie.go:23",
            "\t/tmp/pkg/mod/github.com/gofiber/fiber/v3@v3.0.0-rc.3/encryptcookie.go:23",
        ] {
            assert_eq!(
                GoTestTraceSourceFramesV1::from_module_cache_panic(raw, &module).frame_class(),
                GoTestTraceFrameClassV1::Unresolved,
                "{raw}"
            );
        }
    }
}
