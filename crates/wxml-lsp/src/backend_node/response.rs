//! Parses and maps C1 `AnalyzeResponse` values to the LSP `Backend` DTOs.

use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::marker::PhantomData;

use serde::de::{self, DeserializeSeed, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use wxml_project::ProjectPath;
use wxml_text::{SourceRange, TextRange};
use wxml_types::{
    CapabilityStatus, DefinitionResult, Diagnostic, DiagnosticSeverity, DiagnosticsResult,
    HoverResult, MarkupKind, SourceLocation, SourceSnapshot, TypeQuality,
};

use super::protocol::{PROTOCOL_VERSION, ProtocolError};
use super::request::FragmentInfo;
use crate::project_host::{PreparedView, ProjectHost};

/// Reason attached when component diagnostics were truncated.
pub(crate) const COMPONENT_DIAGNOSTICS_TRUNCATED_REASON: &str =
    "component diagnostics were truncated";

/// Maximum definition targets returned for one request.
pub(crate) const MAX_DEFINITION_LOCATIONS: usize = 32;

/// Maximum component diagnostics accepted in one response.
pub(crate) const MAX_COMPONENT_DIAGNOSTICS: usize = 128;

/// Maximum fragments accepted in one response.
pub(crate) const MAX_RESPONSE_FRAGMENTS: usize = 4_096;

/// Maximum diagnostics accepted in one response.
pub(crate) const MAX_RESPONSE_DIAGNOSTICS: usize = 8_192;

/// Maximum insertions accepted in one response.
pub(crate) const MAX_RESPONSE_INSERTIONS: usize = 4_096;

/// Reason attached when a response was truncated at one of the entry limits.
pub(crate) const RESPONSE_TRUNCATED_REASON: &str =
    "node response exceeded an entry limit; results were truncated";

/// One C1 response.
#[derive(Clone, Debug)]
pub(crate) struct AnalyzeResponse {
    /// Protocol version, must be 2.
    pub protocol_version: u32,
    /// Request id echoed by Node.
    pub request_id: String,
    /// Per-capability status.
    pub states: CapabilityStates,
    /// Insertions; only the revision fields are validated here.
    pub insertions: Vec<Insertion>,
    /// Per-fragment results.
    pub fragments: Vec<FragmentResult>,
    /// Per-component diagnostic results.
    pub components: Vec<ComponentResult>,
    /// Diagnostics or fragments were dropped at the configured entry limits.
    ///
    /// The mapping functions downgrade every capability to `Partial` so a
    /// truncated response is never reported as a complete check.
    pub truncated: bool,
    /// The component diagnostic budget was exhausted.
    ///
    /// Tracked separately so `diagnostics()` can report a component-specific,
    /// path-free partial reason.
    pub components_truncated: bool,
    /// The insertion array exceeded [`MAX_RESPONSE_INSERTIONS`].
    ///
    /// Revisions cannot be validated for dropped insertions, so such a
    /// response is rejected rather than truncated.
    pub insertions_overflow: bool,
}

impl<'de> Deserialize<'de> for AnalyzeResponse {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_map(AnalyzeResponseVisitor)
    }
}

struct AnalyzeResponseVisitor;

impl<'de> Visitor<'de> for AnalyzeResponseVisitor {
    type Value = AnalyzeResponse;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a C1 analyze response")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut protocol_version = None;
        let mut request_id = None;
        let mut states = None;
        let mut insertions = Vec::new();
        let mut fragments = Vec::new();
        let mut components = Vec::new();
        let mut truncated = false;
        let mut insertions_overflow = false;
        let mut components_truncated = false;
        let mut remaining_diagnostics = MAX_RESPONSE_DIAGNOSTICS;
        let mut remaining_component_diagnostics = MAX_COMPONENT_DIAGNOSTICS;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "protocolVersion" => protocol_version = Some(map.next_value()?),
                "requestId" => request_id = Some(map.next_value()?),
                "states" => states = Some(map.next_value()?),
                "insertions" => {
                    let (items, overflow) =
                        map.next_value_seed(CappedVecSeed::<Insertion, MAX_RESPONSE_INSERTIONS>(
                            PhantomData,
                        ))?;
                    insertions = items;
                    insertions_overflow |= overflow;
                }
                "fragments" => {
                    fragments = map.next_value_seed(FragmentsSeed {
                        remaining_diagnostics: &mut remaining_diagnostics,
                        truncated: &mut truncated,
                    })?;
                }
                "components" => {
                    components = map.next_value_seed(ComponentsSeed {
                        remaining: &mut remaining_component_diagnostics,
                        truncated: &mut components_truncated,
                    })?;
                }
                _ => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        Ok(AnalyzeResponse {
            protocol_version: protocol_version
                .ok_or_else(|| de::Error::missing_field("protocolVersion"))?,
            request_id: request_id.ok_or_else(|| de::Error::missing_field("requestId"))?,
            states: states.ok_or_else(|| de::Error::missing_field("states"))?,
            insertions,
            fragments,
            components,
            truncated,
            components_truncated,
            insertions_overflow,
        })
    }
}

/// Deserializes at most `N` entries, consuming and discarding any extra ones.
struct CappedVecSeed<T, const N: usize>(PhantomData<T>);

impl<'de, T, const N: usize> DeserializeSeed<'de> for CappedVecSeed<T, N>
where
    T: Deserialize<'de>,
{
    type Value = (Vec<T>, bool);

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_seq(CappedVecVisitor::<T, N>(PhantomData))
    }
}

struct CappedVecVisitor<T, const N: usize>(PhantomData<T>);

impl<'de, T, const N: usize> Visitor<'de> for CappedVecVisitor<T, N>
where
    T: Deserialize<'de>,
{
    type Value = (Vec<T>, bool);

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a bounded list")
    }

    fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut items = Vec::new();
        let mut overflow = false;
        while let Some(item) = seq.next_element::<T>()? {
            if items.len() < N {
                items.push(item);
            } else {
                overflow = true;
            }
        }
        Ok((items, overflow))
    }
}

struct FragmentsSeed<'a> {
    remaining_diagnostics: &'a mut usize,
    truncated: &'a mut bool,
}

impl<'de> DeserializeSeed<'de> for FragmentsSeed<'_> {
    type Value = Vec<FragmentResult>;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_seq(FragmentsVisitor {
            remaining_diagnostics: self.remaining_diagnostics,
            truncated: self.truncated,
        })
    }
}

struct FragmentsVisitor<'a> {
    remaining_diagnostics: &'a mut usize,
    truncated: &'a mut bool,
}

impl<'de> Visitor<'de> for FragmentsVisitor<'_> {
    type Value = Vec<FragmentResult>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a list of fragments")
    }

    fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut fragments = Vec::new();
        loop {
            if fragments.len() < MAX_RESPONSE_FRAGMENTS {
                let Some(fragment) = seq.next_element_seed(FragmentSeed {
                    remaining_diagnostics: &mut *self.remaining_diagnostics,
                    truncated: &mut *self.truncated,
                })?
                else {
                    break;
                };
                fragments.push(fragment);
            } else {
                let Some(_) = seq.next_element::<IgnoredAny>()? else {
                    break;
                };
                *self.truncated = true;
            }
        }
        Ok(fragments)
    }
}

struct FragmentSeed<'a> {
    remaining_diagnostics: &'a mut usize,
    truncated: &'a mut bool,
}

impl<'de> DeserializeSeed<'de> for FragmentSeed<'_> {
    type Value = FragmentResult;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_map(FragmentVisitor {
            remaining_diagnostics: self.remaining_diagnostics,
            truncated: self.truncated,
        })
    }
}

struct FragmentVisitor<'a> {
    remaining_diagnostics: &'a mut usize,
    truncated: &'a mut bool,
}

impl<'de> Visitor<'de> for FragmentVisitor<'_> {
    type Value = FragmentResult;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a fragment result")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut fragment_id = None;
        let mut hover = None;
        let mut diagnostics = Vec::new();
        let mut definition = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "fragmentId" => fragment_id = Some(map.next_value()?),
                "hover" => hover = map.next_value()?,
                "diagnostics" => {
                    diagnostics = map.next_value_seed(DiagnosticsSeed {
                        remaining: &mut *self.remaining_diagnostics,
                        truncated: &mut *self.truncated,
                    })?;
                }
                "definition" => definition = map.next_value()?,
                _ => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        Ok(FragmentResult {
            fragment_id: fragment_id.ok_or_else(|| de::Error::missing_field("fragmentId"))?,
            hover,
            diagnostics,
            definition,
        })
    }
}

struct DiagnosticsSeed<'a> {
    remaining: &'a mut usize,
    truncated: &'a mut bool,
}

impl<'de> DeserializeSeed<'de> for DiagnosticsSeed<'_> {
    type Value = Vec<DiagnosticWire>;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_seq(DiagnosticsVisitor {
            remaining: self.remaining,
            truncated: self.truncated,
        })
    }
}

struct DiagnosticsVisitor<'a> {
    remaining: &'a mut usize,
    truncated: &'a mut bool,
}

impl<'de> Visitor<'de> for DiagnosticsVisitor<'_> {
    type Value = Vec<DiagnosticWire>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a list of diagnostics")
    }

    fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut diagnostics = Vec::new();
        while let Some(diagnostic) = seq.next_element::<DiagnosticWire>()? {
            if *self.remaining == 0 {
                *self.truncated = true;
                continue;
            }
            *self.remaining -= 1;
            diagnostics.push(diagnostic);
        }
        Ok(diagnostics)
    }
}

struct ComponentsSeed<'a> {
    remaining: &'a mut usize,
    truncated: &'a mut bool,
}

impl<'de> DeserializeSeed<'de> for ComponentsSeed<'_> {
    type Value = Vec<ComponentResult>;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_seq(ComponentsVisitor {
            remaining: self.remaining,
            truncated: self.truncated,
        })
    }
}

struct ComponentsVisitor<'a> {
    remaining: &'a mut usize,
    truncated: &'a mut bool,
}

impl<'de> Visitor<'de> for ComponentsVisitor<'_> {
    type Value = Vec<ComponentResult>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a list of component results")
    }

    fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut components = Vec::new();
        while let Some(component) = seq.next_element_seed(ComponentSeed {
            remaining: &mut *self.remaining,
            truncated: &mut *self.truncated,
        })? {
            components.push(component);
        }
        Ok(components)
    }
}

struct ComponentSeed<'a> {
    remaining: &'a mut usize,
    truncated: &'a mut bool,
}

impl<'de> DeserializeSeed<'de> for ComponentSeed<'_> {
    type Value = ComponentResult;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_map(ComponentVisitor {
            remaining: self.remaining,
            truncated: self.truncated,
        })
    }
}

struct ComponentVisitor<'a> {
    remaining: &'a mut usize,
    truncated: &'a mut bool,
}

impl<'de> Visitor<'de> for ComponentVisitor<'_> {
    type Value = ComponentResult;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a component result")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut tag = None;
        let mut diagnostics = Vec::new();
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "tag" => tag = Some(map.next_value()?),
                "diagnostics" => {
                    diagnostics = map.next_value_seed(ComponentDiagnosticsSeed {
                        remaining: &mut *self.remaining,
                        truncated: &mut *self.truncated,
                    })?;
                }
                _ => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        Ok(ComponentResult {
            tag: tag.ok_or_else(|| de::Error::missing_field("tag"))?,
            diagnostics,
        })
    }
}

struct ComponentDiagnosticsSeed<'a> {
    remaining: &'a mut usize,
    truncated: &'a mut bool,
}

impl<'de> DeserializeSeed<'de> for ComponentDiagnosticsSeed<'_> {
    type Value = Vec<ComponentDiagnosticWire>;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_seq(ComponentDiagnosticsVisitor {
            remaining: self.remaining,
            truncated: self.truncated,
        })
    }
}

struct ComponentDiagnosticsVisitor<'a> {
    remaining: &'a mut usize,
    truncated: &'a mut bool,
}

impl<'de> Visitor<'de> for ComponentDiagnosticsVisitor<'_> {
    type Value = Vec<ComponentDiagnosticWire>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a list of component diagnostics")
    }

    fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut diagnostics = Vec::new();
        while let Some(diagnostic) = seq.next_element::<ComponentDiagnosticWire>()? {
            if *self.remaining == 0 {
                *self.truncated = true;
                continue;
            }
            *self.remaining -= 1;
            diagnostics.push(diagnostic);
        }
        Ok(diagnostics)
    }
}

/// Per-capability statuses.
#[derive(Clone, Debug, Deserialize)]
pub(crate) struct CapabilityStates {
    /// Hover status.
    pub hover: CapabilityStatus,
    /// Diagnostics status.
    pub diagnostics: CapabilityStatus,
    /// Definition status.
    pub definition: CapabilityStatus,
}

/// One insertion record.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Insertion {
    /// View revision supplied by the request.
    pub view_revision: u64,
}

/// One fragment result.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FragmentResult {
    /// Fragment id.
    pub fragment_id: String,
    /// Hover result.
    #[serde(default)]
    pub hover: Option<HoverResultWire>,
    /// Diagnostics for this fragment.
    #[serde(default)]
    pub diagnostics: Vec<DiagnosticWire>,
    /// Definition result.
    #[serde(default)]
    pub definition: Option<DefinitionWire>,
}

/// Hover payload.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HoverResultWire {
    /// Displayed type text.
    #[serde(default)]
    pub type_text: String,
    /// Type-only text.
    #[serde(default)]
    pub type_only: String,
    /// Quality enum.
    #[serde(default)]
    pub quality: String,
    /// Documentation text.
    #[serde(default)]
    pub documentation: String,
}

/// One diagnostic.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DiagnosticWire {
    /// TypeScript code.
    #[serde(default)]
    pub code: i64,
    /// WXML byte range.
    #[serde(default)]
    pub range: Option<WireRange>,
    /// Message.
    #[serde(default)]
    pub message: String,
    /// Attribution enum.
    #[serde(default)]
    pub attribution: String,
}

/// Diagnostics for one custom-component usage.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ComponentResult {
    /// Component tag as written in the template.
    #[allow(dead_code)] // protocol field; diagnostics are not attributed by tag
    pub tag: String,
    /// Diagnostics for this usage.
    #[serde(default)]
    pub diagnostics: Vec<ComponentDiagnosticWire>,
}

/// One component-level diagnostic.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ComponentDiagnosticWire {
    /// WXML byte range; a diagnostic without one is skipped instead of
    /// rejecting the whole response.
    #[serde(default)]
    pub range: Option<WireRange>,
    /// Optional severity; anything other than `warning` is an error.
    #[serde(default)]
    pub severity: Option<String>,
    /// Message.
    #[serde(default)]
    pub message: String,
}

/// Definition payload.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DefinitionWire {
    /// `script`, `generated`, `external` or `dropped`.
    #[serde(default)]
    pub kind: String,
    /// Target file id in the `files[]` identity space.
    #[serde(default)]
    pub file_id: Option<String>,
    /// Target byte range.
    #[serde(default)]
    pub range: Option<WireRange>,
}

/// Wire range.
#[derive(Clone, Copy, Debug, Deserialize)]
pub(crate) struct WireRange {
    /// Start byte.
    pub start: u32,
    /// End byte.
    pub end: u32,
}

impl WireRange {
    fn source_range(self) -> Option<SourceRange> {
        SourceRange::from_bounds(self.start, self.end).ok()
    }
}

/// Parses one response line and validates the envelope.
/// Parses one raw C1 response line (test/helper path).
#[cfg(test)]
pub(crate) fn parse(
    line: &str,
    expected_request: &str,
    expected_view_revision: u64,
) -> Result<AnalyzeResponse, ProtocolError> {
    let value: serde_json::Value = serde_json::from_str(line).map_err(|_| ProtocolError::Json)?;
    parse_value(value, expected_request, expected_view_revision)
}

/// Parses a JSON-RPC `result` value and validates the C1 envelope.
#[cfg(test)]
pub(crate) fn parse_value(
    value: serde_json::Value,
    expected_request: &str,
    expected_view_revision: u64,
) -> Result<AnalyzeResponse, ProtocolError> {
    let response: AnalyzeResponse =
        serde_json::from_value(value).map_err(|_| ProtocolError::Json)?;
    validate(response, expected_request, expected_view_revision)
}

/// Parses one raw JSON-RPC line directly into the C1 response.
///
/// Deserializing straight into [`AnalyzeResponse`] avoids the intermediate
/// `serde_json::Value` tree, so a diagnostics-heavy response cannot create a
/// hundreds-of-MiB peak before the entry limits are applied. Oversized
/// diagnostic and fragment arrays are truncated while parsing and reported via
/// [`AnalyzeResponse::truncated`].
pub(crate) fn parse_line(
    line: &str,
    expected_request: &str,
    expected_view_revision: u64,
) -> Result<AnalyzeResponse, ProtocolError> {
    let envelope: Envelope = serde_json::from_str(line).map_err(|_| ProtocolError::Json)?;
    if envelope.jsonrpc.as_deref() != Some("2.0") {
        return Err(ProtocolError::Shape);
    }
    if envelope.id.as_ref().and_then(serde_json::Value::as_str) != Some(expected_request) {
        return Err(ProtocolError::IdMismatch);
    }
    if let Some(error) = envelope.error {
        let object = error.as_object();
        let code = object
            .and_then(|object| object.get("code"))
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(-32_603);
        let message = crate::handlers::log_safe_text(
            object
                .and_then(|object| object.get("message"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or("node error"),
            512,
        );
        let data_code = object
            .and_then(|object| object.get("data"))
            .and_then(serde_json::Value::as_object)
            .and_then(|data| data.get("code"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned);
        return Err(ProtocolError::Node {
            code,
            message,
            data_code,
        });
    }
    let response = envelope.result.ok_or(ProtocolError::Shape)?;
    validate(response, expected_request, expected_view_revision)
}

/// JSON-RPC envelope for [`parse_line`].
#[derive(Deserialize)]
struct Envelope {
    #[serde(default)]
    jsonrpc: Option<String>,
    #[serde(default)]
    id: Option<serde_json::Value>,
    #[serde(default)]
    error: Option<serde_json::Value>,
    #[serde(default)]
    result: Option<AnalyzeResponse>,
}

/// Validates the C1 fields after deserialization.
fn validate(
    response: AnalyzeResponse,
    expected_request: &str,
    expected_view_revision: u64,
) -> Result<AnalyzeResponse, ProtocolError> {
    if response.insertions_overflow {
        return Err(ProtocolError::Limit);
    }
    if response.protocol_version != u32::try_from(PROTOCOL_VERSION).unwrap_or(1) {
        return Err(ProtocolError::Version);
    }
    if response.request_id != expected_request {
        return Err(ProtocolError::IdMismatch);
    }
    if response
        .insertions
        .iter()
        .any(|insertion| insertion.view_revision != expected_view_revision)
    {
        return Err(ProtocolError::RevisionMismatch);
    }
    Ok(response)
}

/// Whether a point request at `offset` belongs to `range`.
///
/// The end is inclusive so hovering at the right edge of an identifier still
/// resolves; editors commonly report that position when the pointer is on the
/// final glyph.
const fn point_in_range(range: SourceRange, offset: u32) -> bool {
    range.start() <= offset && offset <= range.end()
}

/// Maps a hover response.
pub(crate) fn hover(
    response: &AnalyzeResponse,
    view: &PreparedView,
    request_fragments: &BTreeMap<String, FragmentInfo>,
    offset: u32,
) -> HoverResult {
    let status = truncated_status(status_for(&response.states.hover, view), response.truncated);
    let mut found = None;
    for fragment in &response.fragments {
        let Some(info) = request_fragments.get(&fragment.fragment_id) else {
            continue;
        };
        let Some(range) = info.target_range else {
            continue;
        };
        if !point_in_range(info.expr_range, offset) {
            continue;
        }
        found = Some((fragment, range));
        break;
    }
    let Some((fragment, range)) = found else {
        return HoverResult {
            contents: String::new(),
            markup: MarkupKind::Markdown,
            range: None,
            status,
            quality: TypeQuality::Unknown,
        };
    };
    let Some(hover) = fragment.hover.as_ref() else {
        return HoverResult {
            contents: String::new(),
            markup: MarkupKind::Markdown,
            range: None,
            status,
            quality: TypeQuality::Unknown,
        };
    };
    let type_text = if hover.type_text.is_empty() {
        hover.type_only.as_str()
    } else {
        hover.type_text.as_str()
    };
    let type_text = crate::handlers::code_text(type_text, 512);
    let mut contents = String::new();
    if !type_text.is_empty() {
        let fence = fence_for(&type_text);
        contents.push_str(&fence);
        contents.push_str("ts\n");
        contents.push_str(&type_text);
        contents.push('\n');
        contents.push_str(&fence);
    }
    if !hover.documentation.is_empty() {
        if !contents.is_empty() {
            contents.push_str("\n\n");
        }
        contents.push_str(&crate::handlers::markdown_escape(&hover.documentation));
    }
    HoverResult {
        contents,
        markup: MarkupKind::Markdown,
        range: Some(range),
        status,
        quality: quality(&hover.quality),
    }
}

/// Maps diagnostics, always keeping the P2 syntax diagnostics.
pub(crate) fn diagnostics(
    response: &AnalyzeResponse,
    view: &PreparedView,
    snapshot: &SourceSnapshot,
) -> DiagnosticsResult {
    let status = truncated_status(
        status_for(&response.states.diagnostics, view),
        response.truncated,
    );
    if matches!(status, CapabilityStatus::Disabled { .. }) {
        // `Disabled` means no type diagnostics are published; P2 syntax stays.
        return DiagnosticsResult {
            diagnostics: view.syntax_diagnostics.clone(),
            status,
        };
    }
    let mut mapped = view.syntax_diagnostics.clone();
    let mut seen = HashSet::new();
    let mut seen_messages = HashSet::new();
    for fragment in &response.fragments {
        for diagnostic in &fragment.diagnostics {
            if diagnostic.attribution == "unattributable" {
                continue;
            }
            let Some(range) = diagnostic.range.and_then(WireRange::source_range) else {
                continue;
            };
            let Ok(text_range) = TextRange::new(range.start(), range.end()) else {
                continue;
            };
            let Some(validated) = snapshot.source_range(text_range) else {
                continue;
            };
            if seen.insert((diagnostic.code, validated.start(), validated.end())) {
                seen_messages.insert((
                    diagnostic.message.clone(),
                    validated.start(),
                    validated.end(),
                ));
                mapped.push(Diagnostic {
                    range: validated,
                    severity: DiagnosticSeverity::Error,
                    message: diagnostic.message.clone(),
                });
            }
        }
    }
    for component in &response.components {
        for diagnostic in &component.diagnostics {
            let Some(range) = diagnostic
                .range
                .as_ref()
                .and_then(|range| range.source_range())
            else {
                continue;
            };
            let Ok(text_range) = TextRange::new(range.start(), range.end()) else {
                continue;
            };
            let Some(validated) = snapshot.source_range(text_range) else {
                continue;
            };
            if !seen_messages.insert((
                diagnostic.message.clone(),
                validated.start(),
                validated.end(),
            )) {
                continue;
            }
            mapped.push(Diagnostic {
                range: validated,
                severity: if diagnostic.severity.as_deref() == Some("warning") {
                    DiagnosticSeverity::Warning
                } else {
                    DiagnosticSeverity::Error
                },
                message: diagnostic.message.clone(),
            });
        }
    }
    let status = if response.components_truncated {
        force_partial_reason(status, COMPONENT_DIAGNOSTICS_TRUNCATED_REASON)
    } else {
        status
    };
    DiagnosticsResult {
        diagnostics: mapped,
        status,
    }
}

/// Maps a definition response.
pub(crate) fn definition(
    response: &AnalyzeResponse,
    view: &PreparedView,
    request_fragments: &BTreeMap<String, FragmentInfo>,
    offset: u32,
    host: &dyn ProjectHost,
) -> DefinitionResult {
    let mut status = truncated_status(
        status_for(&response.states.definition, view),
        response.truncated,
    );
    let mut locations = Vec::new();
    let mut seen = HashSet::new();
    let mut dropped = false;
    let mut duplicates = false;
    let mut truncated = false;
    for fragment in &response.fragments {
        let Some(info) = request_fragments.get(&fragment.fragment_id) else {
            continue;
        };
        if info.target_range.is_none() || !point_in_range(info.expr_range, offset) {
            continue;
        }
        let Some(definition) = fragment.definition.as_ref() else {
            continue;
        };
        if definition.kind == "dropped" {
            dropped = true;
            continue;
        }
        if !matches!(
            definition.kind.as_str(),
            "script" | "generated" | "external"
        ) {
            dropped = true;
            continue;
        }
        let (Some(file_id), Some(range)) = (definition.file_id.as_ref(), definition.range) else {
            dropped = true;
            continue;
        };
        let Some(path) = ProjectPath::new(file_id.clone()).ok() else {
            dropped = true;
            continue;
        };
        let Ok(file) = host.file_id(&path) else {
            dropped = true;
            continue;
        };
        let Some(range) = range.source_range() else {
            dropped = true;
            continue;
        };
        if !seen.insert((file.clone(), range.start(), range.end())) {
            duplicates = true;
            continue;
        }
        if locations.len() >= MAX_DEFINITION_LOCATIONS {
            truncated = true;
            break;
        }
        locations.push(SourceLocation { file, range });
    }
    if dropped {
        status = force_partial_reason(status, "definition target could not be mapped");
    }
    if duplicates {
        status = force_partial_reason(status, "duplicate definition targets were dropped");
    }
    if truncated {
        status = force_partial_reason(
            status,
            "definition location limit reached; later targets were dropped",
        );
    }
    DefinitionResult { locations, status }
}

fn status_for(base: &CapabilityStatus, view: &PreparedView) -> CapabilityStatus {
    merge_status(base.clone(), view.status_hint.as_ref())
}

fn merge_status(base: CapabilityStatus, hint: Option<&CapabilityStatus>) -> CapabilityStatus {
    let Some(hint) = hint else {
        return base;
    };
    if rank(hint) > rank(&base) {
        hint.clone()
    } else {
        base
    }
}

/// Downgrades a complete status to `Partial` when the response was truncated.
fn truncated_status(status: CapabilityStatus, truncated: bool) -> CapabilityStatus {
    if truncated {
        force_partial_reason(status, RESPONSE_TRUNCATED_REASON)
    } else {
        status
    }
}

fn force_partial_reason(status: CapabilityStatus, reason: &'static str) -> CapabilityStatus {
    match status {
        CapabilityStatus::Disabled { .. }
        | CapabilityStatus::Unsupported { .. }
        | CapabilityStatus::Unavailable { .. } => status,
        _ => CapabilityStatus::Partial {
            reason: reason.to_owned(),
        },
    }
}

#[allow(clippy::match_same_arms)] // `_` covers future non_exhaustive variants
const fn rank(status: &CapabilityStatus) -> u8 {
    match status {
        CapabilityStatus::Unavailable { .. } => 5,
        CapabilityStatus::Disabled { .. } => 4,
        CapabilityStatus::Unsupported { .. } => 3,
        CapabilityStatus::Unknown { .. } => 2,
        CapabilityStatus::Partial { .. } => 1,
        CapabilityStatus::Supported => 0,
        _ => 2,
    }
}

/// Chooses a code fence longer than any backtick run in `text`.
fn fence_for(text: &str) -> String {
    let mut longest = 0_usize;
    let mut run = 0_usize;
    for character in text.chars() {
        if character == '`' {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    let length = if longest >= 3 { longest + 1 } else { 3 };
    "`".repeat(length)
}

fn quality(value: &str) -> TypeQuality {
    match value {
        "known" => TypeQuality::Known,
        "any" => TypeQuality::Any,
        "missing" => TypeQuality::Missing,
        _ => TypeQuality::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;
    use std::sync::Arc;

    use wxml_types::{FileId, ProjectId, SnapshotId, SourceRevision};

    use super::*;

    fn envelope(fragments: &serde_json::Value) -> String {
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": "r1",
            "result": {
                "protocolVersion": 2,
                "requestId": "r1",
                "states": {
                    "hover": {"status": "supported"},
                    "diagnostics": {"status": "supported"},
                    "definition": {"status": "supported"}
                },
                "insertions": [{"viewRevision": 1}],
                "fragments": fragments
            }
        })
        .to_string()
    }

    fn five_diagnostics() -> serde_json::Value {
        serde_json::json!([
            {"code": 1, "message": "a", "attribution": "source"},
            {"code": 2, "message": "b", "attribution": "source"},
            {"code": 3, "message": "c", "attribution": "source"},
            {"code": 4, "message": "d", "attribution": "source"},
            {"code": 5, "message": "e", "attribution": "source"}
        ])
    }

    #[test]
    fn oversized_diagnostics_are_truncated_not_rejected() {
        let diagnostics = (0..MAX_RESPONSE_DIAGNOSTICS + 10)
            .map(|index| serde_json::json!({"code": index, "message": "x"}))
            .collect::<Vec<_>>();
        let line = envelope(&serde_json::json!([
            {"fragmentId": "f0", "diagnostics": diagnostics}
        ]));
        let response = parse_line(&line, "r1", 1).expect("truncated response");
        assert!(response.truncated);
        assert_eq!(
            response.fragments[0].diagnostics.len(),
            MAX_RESPONSE_DIAGNOSTICS
        );
    }

    #[test]
    fn many_fragments_with_diagnostics_are_truncated_not_rejected() {
        let fragments = (0..2_048)
            .map(|index| {
                serde_json::json!({
                    "fragmentId": format!("f{index}"),
                    "diagnostics": five_diagnostics()
                })
            })
            .collect::<Vec<_>>();
        let line = envelope(&serde_json::Value::Array(fragments));
        let response = parse_line(&line, "r1", 1).expect("accepted response");
        assert!(response.truncated);
        assert_eq!(response.fragments.len(), 2_048);
        let total = response
            .fragments
            .iter()
            .map(|fragment| fragment.diagnostics.len())
            .sum::<usize>();
        assert_eq!(total, MAX_RESPONSE_DIAGNOSTICS);
    }

    #[test]
    fn legal_large_response_is_not_rejected() {
        let fragments = (0..2_048)
            .map(|index| {
                serde_json::json!({
                    "fragmentId": format!("f{index}"),
                    "diagnostics": [
                        {"code": 1, "message": "a"},
                        {"code": 2, "message": "b"},
                        {"code": 3, "message": "c"},
                        {"code": 4, "message": "d"}
                    ]
                })
            })
            .collect::<Vec<_>>();
        let line = envelope(&serde_json::Value::Array(fragments));
        let response = parse_line(&line, "r1", 1).expect("valid response");
        assert!(!response.truncated);
        let total = response
            .fragments
            .iter()
            .map(|fragment| fragment.diagnostics.len())
            .sum::<usize>();
        assert_eq!(total, MAX_RESPONSE_DIAGNOSTICS);
    }

    #[test]
    fn large_diagnostics_line_is_truncated_without_a_value_tree() {
        let message = "x".repeat(100);
        let mut diagnostics = String::new();
        for index in 0..40_000_u32 {
            if index != 0 {
                diagnostics.push(',');
            }
            write!(
                diagnostics,
                "{{\"code\":{index},\"message\":\"{message}\"}}"
            )
            .expect("write");
        }
        let line = format!(
            r#"{{"jsonrpc":"2.0","id":"r1","result":{{"protocolVersion":2,"requestId":"r1","states":{{"hover":{{"status":"supported"}},"diagnostics":{{"status":"supported"}},"definition":{{"status":"supported"}}}},"insertions":[{{"viewRevision":1}}],"fragments":[{{"fragmentId":"f0","diagnostics":[{diagnostics}]}}]}}}}"#
        );
        assert!(line.len() < crate::backend_node::config::DEFAULT_MAX_LINE_BYTES);
        let response = parse_line(&line, "r1", 1).expect("truncated response");
        assert!(response.truncated);
        assert_eq!(
            response.fragments[0].diagnostics.len(),
            MAX_RESPONSE_DIAGNOSTICS
        );
    }

    #[test]
    fn oversized_fragment_array_is_truncated() {
        let fragments = (0..=MAX_RESPONSE_FRAGMENTS)
            .map(|index| serde_json::json!({"fragmentId": format!("f{index}")}))
            .collect::<Vec<_>>();
        let line = envelope(&serde_json::Value::Array(fragments));
        let response = parse_line(&line, "r1", 1).expect("truncated fragments");
        assert!(response.truncated);
        assert_eq!(response.fragments.len(), MAX_RESPONSE_FRAGMENTS);
    }

    #[test]
    fn oversized_insertion_array_is_rejected() {
        let insertions = (0..=MAX_RESPONSE_INSERTIONS)
            .map(|_| serde_json::json!({"viewRevision": 1}))
            .collect::<Vec<_>>();
        let value = serde_json::json!({
            "protocolVersion": 2,
            "requestId": "r1",
            "states": {
                "hover": {"status": "supported"},
                "diagnostics": {"status": "supported"},
                "definition": {"status": "supported"}
            },
            "insertions": insertions,
            "fragments": []
        });
        assert!(matches!(
            parse_value(value, "r1", 1),
            Err(ProtocolError::Limit)
        ));
    }

    #[test]
    fn rejects_wrong_protocol_and_revision() {
        let line = r#"{
            "protocolVersion": 2,
            "requestId": "r1",
            "jsCheckApplied": "pragma",
            "states": {"hover": {"status": "supported"}, "diagnostics": {"status": "supported"}, "definition": {"status": "supported"}},
            "insertions": [{"viewRevision": 7, "backendFileVersion": 1}],
            "fragments": []
        }"#;
        assert!(parse(line, "r1", 7).is_ok());
        assert!(matches!(
            parse(line, "r1", 8),
            Err(ProtocolError::RevisionMismatch)
        ));
        assert!(matches!(
            parse(line, "r2", 7),
            Err(ProtocolError::IdMismatch)
        ));
        let wrong = line.replace("\"protocolVersion\": 2", "\"protocolVersion\": 3");
        assert!(matches!(
            parse(&wrong, "r1", 7),
            Err(ProtocolError::Version)
        ));
    }

    #[test]
    fn v2_reserved_response_fields_are_ignored() {
        let line = r#"{
            "protocolVersion": 2,
            "requestId": "r1",
            "states": {"hover": {"status": "supported"}, "diagnostics": {"status": "supported"}, "definition": {"status": "supported"}},
            "insertions": [{"viewRevision": 7, "backendFileVersion": 1}],
            "fragments": [{"fragmentId": "f0", "eventResults": [{"eventId": "e0"}]}],
            "eventResults": [{"eventId": "e0", "state": "supported"}],
            "components": [{"tag": "card", "diagnostics": []}]
        }"#;
        let response = parse(line, "r1", 7).expect("v2 response");
        assert_eq!(response.fragments.len(), 1);
        assert_eq!(response.fragments[0].fragment_id, "f0");
        assert_eq!(response.components.len(), 1);
        assert_eq!(response.components[0].tag, "card");
    }

    fn envelope_with_components(components: &serde_json::Value) -> String {
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": "r1",
            "result": {
                "protocolVersion": 2,
                "requestId": "r1",
                "states": {
                    "hover": {"status": "supported"},
                    "diagnostics": {"status": "supported"},
                    "definition": {"status": "supported"}
                },
                "insertions": [{"viewRevision": 1}],
                "fragments": [],
                "components": components
            }
        })
        .to_string()
    }

    fn prepared_view() -> PreparedView {
        PreparedView {
            view_id: "pages/index/index.wxml:1".to_owned(),
            target: None,
            files: Arc::from(Vec::new()),
            compiler_options: serde_json::Map::new(),
            fragments: Arc::from(Vec::new()),
            components: Arc::from(Vec::new()),
            project_root: std::path::PathBuf::new(),
            syntax_diagnostics: Vec::new(),
            status_hint: None,
        }
    }

    fn snapshot() -> SourceSnapshot {
        SourceSnapshot::new(
            FileId::new(ProjectId::new("p"), "pages/index/index.wxml").expect("file"),
            SourceRevision::new(1),
            SnapshotId::new(1),
            "{{ user.name }}",
        )
    }

    #[test]
    fn component_diagnostics_are_mapped_with_severity() {
        let line = envelope_with_components(&serde_json::json!([
            {
                "tag": "card",
                "diagnostics": [
                    {"range": {"start": 3, "end": 8}, "message": "expects string", "severity": "error"},
                    {"range": {"start": 3, "end": 8}, "message": "soft", "severity": "warning"}
                ]
            }
        ]));
        let response = parse_line(&line, "r1", 1).expect("component response");
        let result = diagnostics(&response, &prepared_view(), &snapshot());
        assert_eq!(result.diagnostics.len(), 2);
        assert_eq!(result.diagnostics[0].severity, DiagnosticSeverity::Error);
        assert_eq!(result.diagnostics[1].severity, DiagnosticSeverity::Warning);
        assert_eq!(result.diagnostics[1].message, "soft");
        assert!(result.status.is_supported());
    }

    #[test]
    fn component_diagnostics_dedupe_and_drop_invalid_ranges() {
        let line = envelope_with_components(&serde_json::json!([
            {
                "tag": "card",
                "diagnostics": [
                    {"range": {"start": 3, "end": 8}, "message": "dupe"},
                    {"range": {"start": 3, "end": 8}, "message": "dupe"},
                    {"range": {"start": 900, "end": 901}, "message": "outside"}
                ]
            }
        ]));
        let response = parse_line(&line, "r1", 1).expect("component response");
        let result = diagnostics(&response, &prepared_view(), &snapshot());
        assert_eq!(result.diagnostics.len(), 1);
        assert_eq!(result.diagnostics[0].message, "dupe");
    }

    #[test]
    fn component_diagnostic_truncation_forces_partial() {
        let entries = (0..=MAX_COMPONENT_DIAGNOSTICS)
            .map(|_| {
                serde_json::json!({
                    "range": {"start": 3, "end": 8},
                    "message": "x"
                })
            })
            .collect::<Vec<_>>();
        let line = envelope_with_components(&serde_json::json!([
            {"tag": "card", "diagnostics": entries}
        ]));
        let response = parse_line(&line, "r1", 1).expect("truncated component response");
        assert!(response.components_truncated);
        assert_eq!(
            response.components[0].diagnostics.len(),
            MAX_COMPONENT_DIAGNOSTICS
        );
        let result = diagnostics(&response, &prepared_view(), &snapshot());
        let CapabilityStatus::Partial { reason } = result.status else {
            panic!("expected partial");
        };
        assert_eq!(reason, COMPONENT_DIAGNOSTICS_TRUNCATED_REASON);
    }
}
