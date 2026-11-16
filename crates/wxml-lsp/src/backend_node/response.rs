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
use super::request::{EventInfo, FragmentInfo};
use crate::project_host::{PreparedView, ProjectHost};

/// Reason attached when component diagnostics were truncated.
pub(crate) const COMPONENT_DIAGNOSTICS_TRUNCATED_REASON: &str =
    "component diagnostics were truncated";

/// Maximum definition targets returned for one request.
pub(crate) const MAX_DEFINITION_LOCATIONS: usize = 32;

/// Maximum per-identifier hover candidates accepted for one fragment.
pub(crate) const MAX_HOVER_CANDIDATES: usize = 32;

/// Maximum component diagnostics accepted in one response.
pub(crate) const MAX_COMPONENT_DIAGNOSTICS: usize = 128;

/// Maximum event results accepted in one response.
///
/// Mirrors `MAX_PREPARED_EVENTS` on the request side; anything beyond it means
/// the response does not match the request and is reported as partial.
pub(crate) const MAX_EVENT_RESULTS: usize = 1024;

/// Reason attached when the event result array exceeded [`MAX_EVENT_RESULTS`].
pub(crate) const EVENT_RESULTS_TRUNCATED_REASON: &str = "event results were truncated";

/// Reason attached when a result names an event that was not requested.
pub(crate) const UNKNOWN_EVENT_RESULT_REASON: &str =
    "a result for an event that was not requested was ignored";

/// Reason attached when the requested event has no result in the response.
pub(crate) const MISSING_EVENT_RESULT_REASON: &str = "the event handler result is missing";

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
#[allow(clippy::struct_excessive_bools)] // one flag per independent truncation source
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
    /// Per-event results for the events named in the request.
    pub event_results: Vec<EventResultWire>,
    /// Diagnostics or fragments were dropped at the configured entry limits.
    ///
    /// The mapping functions downgrade every capability to `Partial` so a
    /// truncated response is never reported as a complete check.
    pub truncated: bool,
    /// The event result array exceeded [`MAX_EVENT_RESULTS`].
    pub events_truncated: bool,
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
        let mut event_results = Vec::new();
        let mut truncated = false;
        let mut events_truncated = false;
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
                "eventResults" => {
                    let (items, overflow) =
                        map.next_value_seed(CappedVecSeed::<EventResultWire, MAX_EVENT_RESULTS>(
                            PhantomData,
                        ))?;
                    event_results = items;
                    events_truncated |= overflow;
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
            event_results,
            truncated,
            events_truncated,
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
        let mut hovers = Vec::new();
        let mut remaining_hovers = MAX_HOVER_CANDIDATES;
        let mut diagnostics = Vec::new();
        let mut definition = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "fragmentId" => fragment_id = Some(map.next_value()?),
                "hover" => hover = map.next_value()?,
                "hovers" => {
                    hovers = map.next_value_seed(HoverCandidatesSeed {
                        remaining: &mut remaining_hovers,
                    })?;
                }
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
            hovers,
            diagnostics,
            definition,
        })
    }
}

struct HoverCandidatesSeed<'a> {
    remaining: &'a mut usize,
}

impl<'de> DeserializeSeed<'de> for HoverCandidatesSeed<'_> {
    type Value = Vec<HoverCandidateWire>;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_seq(self)
    }
}

impl<'de> Visitor<'de> for HoverCandidatesSeed<'_> {
    type Value = Vec<HoverCandidateWire>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a hover candidate array")
    }

    fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut candidates = Vec::new();
        while let Some(candidate) = seq.next_element::<HoverCandidateWire>()? {
            if *self.remaining == 0 {
                continue;
            }
            *self.remaining -= 1;
            candidates.push(candidate);
        }
        Ok(candidates)
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
    /// Hover result for the fragment's default target.
    #[serde(default)]
    pub hover: Option<HoverResultWire>,
    /// Per-identifier hover results (protocol v2).
    #[serde(default)]
    pub hovers: Vec<HoverCandidateWire>,
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

/// Hover payload attached to one identifier range.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HoverCandidateWire {
    /// WXML byte range of the identifier.
    #[serde(default)]
    pub range: Option<WireRange>,
    /// Hover fields for the identifier.
    #[serde(flatten)]
    pub hover: HoverResultWire,
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

/// Resolution result for one requested event binding.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EventResultWire {
    /// Request event id, for example `e0`.
    pub event_id: String,
    /// Per-event status; absent means the backend did not classify it.
    #[serde(default)]
    pub state: Option<EventStateWire>,
    /// Method declaration, absent when the handler does not resolve.
    #[serde(default)]
    pub definition: Option<DefinitionWire>,
    /// Missing-handler diagnostic; present only for a supported event whose
    /// handler could not be found.
    #[serde(default)]
    pub diagnostic: Option<ComponentDiagnosticWire>,
    /// One-line method signature, used by event-handler hover.
    #[serde(default)]
    pub signature: Option<String>,
}

/// Status of one event result.
///
/// The protocol writes this as `{"status": ..., "reason": ...}`; a bare status
/// string is accepted as well so an older backend keeps working.
#[derive(Clone, Debug)]
pub(crate) struct EventStateWire {
    /// `supported`, `partial`, `unsupported`, `unavailable` or `disabled`.
    pub status: String,
    /// Optional explanation for a degraded status.
    pub reason: Option<String>,
}

impl<'de> Deserialize<'de> for EventStateWire {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            /// `"state": "supported"`.
            Name(String),
            /// `"state": {"status": "partial", "reason": "..."}`.
            Detail {
                #[serde(default)]
                status: String,
                #[serde(default)]
                reason: Option<String>,
            },
        }
        Ok(match Repr::deserialize(deserializer)? {
            Repr::Name(status) => Self {
                status,
                reason: None,
            },
            Repr::Detail { status, reason } => Self { status, reason },
        })
    }
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
    request_events: &[EventInfo],
    offset: u32,
) -> HoverResult {
    let status = truncated_status(status_for(&response.states.hover, view), response.truncated);
    if let Some(info) = request_events
        .iter()
        .find(|info| point_in_range(info.handler_range, offset))
    {
        return event_hover(response, info, request_events, status);
    }
    let mut found = None;
    for fragment in &response.fragments {
        let Some(info) = request_fragments.get(&fragment.fragment_id) else {
            continue;
        };
        if !point_in_range(info.expr_range, offset) {
            continue;
        }
        // Protocol v2 reports every identifier in the expression. Pick the
        // identifier under the pointer so `{{ user.name }}` answers `user`
        // with `user`'s type instead of the expression's default target.
        if let Some((range, hover)) = fragment.hovers.iter().find_map(|candidate| {
            let range = candidate.range?.source_range()?;
            point_in_range(range, offset).then_some((range, &candidate.hover))
        }) {
            found = Some((range, hover));
            break;
        }
        // Backward compatibility: an older backend emits only the single
        // default-target hover. Keep answering anywhere inside the expression.
        if fragment.hovers.is_empty() {
            if let (Some(range), Some(hover)) = (info.target_range, fragment.hover.as_ref()) {
                found = Some((range, hover));
                break;
            }
        }
    }
    let Some((range, hover)) = found else {
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

/// Hovers an event handler with the signature reported by the backend.
fn event_hover(
    response: &AnalyzeResponse,
    info: &EventInfo,
    request_events: &[EventInfo],
    mut status: CapabilityStatus,
) -> HoverResult {
    status = event_scope_status(response, request_events, status);
    let Some(result) = response
        .event_results
        .iter()
        .find(|result| result.event_id == info.event_id)
    else {
        return HoverResult {
            contents: String::new(),
            markup: MarkupKind::Markdown,
            range: None,
            status: force_partial_reason(status, MISSING_EVENT_RESULT_REASON),
            quality: TypeQuality::Unknown,
        };
    };
    status = merge_status(status, Some(&event_state_status(result.state.as_ref())));
    let signature = result
        .signature
        .as_deref()
        .map(str::trim)
        .filter(|signature| !signature.is_empty());
    let Some(signature) = signature else {
        return HoverResult {
            contents: String::new(),
            markup: MarkupKind::Markdown,
            range: None,
            status,
            quality: TypeQuality::Unknown,
        };
    };
    let signature = crate::handlers::code_text(signature, 512);
    if signature.is_empty() {
        return HoverResult {
            contents: String::new(),
            markup: MarkupKind::Markdown,
            range: None,
            status,
            quality: TypeQuality::Unknown,
        };
    }
    let fence = fence_for(&signature);
    HoverResult {
        contents: format!("{fence}ts\n{signature}\n{fence}"),
        markup: MarkupKind::Markdown,
        range: Some(info.handler_range),
        status,
        quality: TypeQuality::Known,
    }
}

/// Maps diagnostics, always keeping the P2 syntax diagnostics.
pub(crate) fn diagnostics(
    response: &AnalyzeResponse,
    view: &PreparedView,
    snapshot: &SourceSnapshot,
    request_events: &[EventInfo],
) -> DiagnosticsResult {
    let mut status = truncated_status(
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
                severity: event_severity(diagnostic.severity.as_deref()),
                message: diagnostic.message.clone(),
            });
        }
    }

    apply_event_results(
        response,
        request_events,
        snapshot,
        &mut mapped,
        &mut seen_messages,
        &mut status,
    );
    if response.components_truncated {
        status = force_partial_reason(status, COMPONENT_DIAGNOSTICS_TRUNCATED_REASON);
    }
    DiagnosticsResult {
        diagnostics: mapped,
        status,
    }
}

/// Appends event-result diagnostics and folds their statuses into `status`.
fn apply_event_results(
    response: &AnalyzeResponse,
    request_events: &[EventInfo],
    snapshot: &SourceSnapshot,
    mapped: &mut Vec<Diagnostic>,
    seen_messages: &mut HashSet<(String, u32, u32)>,
    status: &mut CapabilityStatus,
) {
    let by_id: BTreeMap<&str, &EventInfo> = request_events
        .iter()
        .map(|info| (info.event_id.as_str(), info))
        .collect();
    let mut unknown_event = false;
    let mut answered = HashSet::with_capacity(request_events.len());
    for result in &response.event_results {
        if !by_id.contains_key(result.event_id.as_str()) {
            // A result must belong to this request; anything else is not a
            // diagnostic for this document.
            unknown_event = true;
            continue;
        }
        answered.insert(result.event_id.as_str());
        // Merge every state before diagnostics are budgeted so a later
        // unsupported/partial event is never hidden by truncation.
        *status = merge_status(
            status.clone(),
            Some(&event_state_status(result.state.as_ref())),
        );
    }
    if request_events
        .iter()
        .any(|info| !answered.contains(info.event_id.as_str()))
    {
        *status = force_partial_reason(status.clone(), MISSING_EVENT_RESULT_REASON);
    }

    let mut invalid_event_range = false;
    let mut truncated_events = false;
    for result in &response.event_results {
        let Some(info) = by_id.get(result.event_id.as_str()) else {
            continue;
        };
        let Some(diagnostic) = result.diagnostic.as_ref() else {
            continue;
        };
        let Some(range) = diagnostic
            .range
            .as_ref()
            .and_then(|range| range.source_range())
        else {
            invalid_event_range = true;
            continue;
        };
        let Ok(text_range) = TextRange::new(range.start(), range.end()) else {
            invalid_event_range = true;
            continue;
        };
        let Some(validated) = snapshot.source_range(text_range) else {
            invalid_event_range = true;
            continue;
        };
        // Event diagnostics belong to the handler token. A backend that points
        // elsewhere is clamped to the requested range and downgraded.
        let contained = info.handler_range.start() <= validated.start()
            && validated.end() <= info.handler_range.end();
        let range = if contained {
            validated
        } else {
            invalid_event_range = true;
            info.handler_range
        };
        if !seen_messages.insert((diagnostic.message.clone(), range.start(), range.end())) {
            continue;
        }
        if mapped.len() >= MAX_RESPONSE_DIAGNOSTICS {
            truncated_events = true;
            break;
        }
        mapped.push(Diagnostic {
            range,
            severity: event_severity(diagnostic.severity.as_deref()),
            message: diagnostic.message.clone(),
        });
    }
    if response.events_truncated {
        *status = force_partial_reason(status.clone(), EVENT_RESULTS_TRUNCATED_REASON);
    }
    if unknown_event {
        *status = force_partial_reason(status.clone(), UNKNOWN_EVENT_RESULT_REASON);
    }
    if invalid_event_range {
        *status = force_partial_reason(
            status.clone(),
            "an event diagnostic range could not be mapped",
        );
    }
    if truncated_events {
        *status = force_partial_reason(status.clone(), "event diagnostics were truncated");
    }
}

/// Maps an event or component severity string; anything but `warning` is an
/// error.
fn event_severity(severity: Option<&str>) -> DiagnosticSeverity {
    if severity == Some("warning") {
        DiagnosticSeverity::Warning
    } else {
        DiagnosticSeverity::Error
    }
}

/// Applies response-level event degradations shared by hover and definition.
fn event_scope_status(
    response: &AnalyzeResponse,
    request_events: &[EventInfo],
    mut status: CapabilityStatus,
) -> CapabilityStatus {
    if response.events_truncated {
        status = force_partial_reason(status, EVENT_RESULTS_TRUNCATED_REASON);
    }
    let requested: HashSet<&str> = request_events
        .iter()
        .map(|info| info.event_id.as_str())
        .collect();
    if response
        .event_results
        .iter()
        .any(|result| !requested.contains(result.event_id.as_str()))
    {
        status = force_partial_reason(status, UNKNOWN_EVENT_RESULT_REASON);
    }
    status
}

/// Maps a reported event status onto a capability status.
fn event_state_status(state: Option<&EventStateWire>) -> CapabilityStatus {
    let Some(state) = state else {
        return CapabilityStatus::Unknown {
            reason: Some("the backend did not report an event status".to_owned()),
        };
    };
    let reason = state.reason.clone().filter(|reason| !reason.is_empty());
    match state.status.as_str() {
        "supported" => CapabilityStatus::Supported,
        "partial" => CapabilityStatus::Partial {
            reason: reason.unwrap_or_else(|| "event handler analysis is partial".to_owned()),
        },
        "unsupported" => CapabilityStatus::Unsupported {
            reason: reason
                .unwrap_or_else(|| "this event binding is not analyzed by the backend".to_owned()),
        },
        "unavailable" => CapabilityStatus::Unavailable {
            reason: reason
                .unwrap_or_else(|| "the type backend could not analyze this event".to_owned()),
        },
        "disabled" => CapabilityStatus::Disabled {
            reason: reason.unwrap_or_else(|| "event handler analysis is disabled".to_owned()),
        },
        _ => CapabilityStatus::Unknown { reason },
    }
}

/// Maps a definition response.
pub(crate) fn definition(
    response: &AnalyzeResponse,
    view: &PreparedView,
    request_fragments: &BTreeMap<String, FragmentInfo>,
    request_events: &[EventInfo],
    offset: u32,
    host: &dyn ProjectHost,
) -> DefinitionResult {
    let mut status = truncated_status(
        status_for(&response.states.definition, view),
        response.truncated,
    );
    if let Some(info) = request_events
        .iter()
        .find(|info| point_in_range(info.handler_range, offset))
    {
        // Event offsets belong to the attribute, not to a mustache expression,
        // so they must never fall through to fragment definitions.
        let script_file = view.target.as_ref().map(|target| target.script_file.path());
        return event_definition(response, request_events, info, status, script_file, host);
    }
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

/// Maps the definition of one requested event handler.
///
/// The event result is authoritative: a missing definition is an empty,
/// *answered* result, not a backend failure.
fn event_definition(
    response: &AnalyzeResponse,
    request_events: &[EventInfo],
    info: &EventInfo,
    mut status: CapabilityStatus,
    script_file: Option<&str>,
    host: &dyn ProjectHost,
) -> DefinitionResult {
    status = event_scope_status(response, request_events, status);
    let Some(result) = response
        .event_results
        .iter()
        .find(|result| result.event_id == info.event_id)
    else {
        return DefinitionResult {
            locations: Vec::new(),
            status: force_partial_reason(status, MISSING_EVENT_RESULT_REASON),
        };
    };
    status = merge_status(status, Some(&event_state_status(result.state.as_ref())));
    let Some(definition) = result.definition.as_ref() else {
        // Checked and answered: the handler has no declaration.
        return DefinitionResult {
            locations: Vec::new(),
            status,
        };
    };
    // Event definitions are `{fileId, range}` in the frozen wire contract; an
    // explicit `kind` is still honored when a backend sends one.
    let kind = definition.kind.as_str();
    let unexpected_file = definition
        .file_id
        .as_deref()
        .is_some_and(|file_id| script_file.is_some_and(|expected| file_id != expected));
    if unexpected_file {
        return DefinitionResult {
            locations: Vec::new(),
            status: force_partial_reason(
                status,
                "an event definition points at an unexpected file",
            ),
        };
    }
    let mapped = if kind.is_empty() || matches!(kind, "script" | "generated" | "external") {
        definition
            .file_id
            .as_ref()
            .zip(definition.range)
            .and_then(|(file_id, range)| {
                let path = ProjectPath::new(file_id.clone()).ok()?;
                let file = host.file_id(&path).ok()?;
                let range = range.source_range()?;
                Some(SourceLocation { file, range })
            })
    } else {
        None
    };
    let Some(location) = mapped else {
        return DefinitionResult {
            locations: Vec::new(),
            status: force_partial_reason(status, "definition target could not be mapped"),
        };
    };
    // One handler resolves to at most one declaration, so the shared location
    // cap and duplicate filter are satisfied by construction.
    DefinitionResult {
        locations: vec![location],
        status,
    }
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
#[allow(clippy::assert_is_empty)]
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
            events: Arc::from(Vec::new()),
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
        let result = diagnostics(&response, &prepared_view(), &snapshot(), &[]);
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
        let result = diagnostics(&response, &prepared_view(), &snapshot(), &[]);
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
        let result = diagnostics(&response, &prepared_view(), &snapshot(), &[]);
        let CapabilityStatus::Partial { reason } = result.status else {
            panic!("expected partial");
        };
        assert_eq!(reason, COMPONENT_DIAGNOSTICS_TRUNCATED_REASON);
    }

    fn envelope_with_events(events: &serde_json::Value) -> String {
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
                "eventResults": events
            }
        })
        .to_string()
    }

    fn event_info(event_id: &str) -> EventInfo {
        EventInfo {
            event_id: event_id.to_owned(),
            attribute: "bindtap".to_owned(),
            event: "tap".to_owned(),
            handler: "onTap".to_owned(),
            attribute_range: SourceRange::from_bounds(0, 15).expect("range"),
            handler_range: SourceRange::from_bounds(2, 12).expect("range"),
        }
    }

    #[test]
    fn event_diagnostics_are_mapped_and_deduped() {
        let line = envelope_with_events(&serde_json::json!([
            {
                "eventId": "e0",
                "state": {"status": "supported"},
                "diagnostic": {
                    "range": {"start": 3, "end": 12},
                    "severity": "error",
                    "message": "Cannot find name 'onMissing' for event tap"
                }
            },
            {
                "eventId": "e0",
                "state": {"status": "supported"},
                "diagnostic": {
                    "range": {"start": 3, "end": 12},
                    "severity": "error",
                    "message": "Cannot find name 'onMissing' for event tap"
                }
            },
            {
                "eventId": "e0",
                "state": {"status": "supported"},
                "diagnostic": {
                    "range": {"start": 3, "end": 8},
                    "severity": "warning",
                    "message": "soft"
                }
            }
        ]));
        let response = parse_line(&line, "r1", 1).expect("event response");
        let result = diagnostics(
            &response,
            &prepared_view(),
            &snapshot(),
            &[event_info("e0")],
        );
        assert_eq!(result.diagnostics.len(), 2);
        assert_eq!(result.diagnostics[0].severity, DiagnosticSeverity::Error);
        assert_eq!(
            result.diagnostics[0].message,
            "Cannot find name 'onMissing' for event tap"
        );
        assert_eq!(result.diagnostics[1].severity, DiagnosticSeverity::Warning);
        assert_eq!(result.diagnostics[1].message, "soft");
        assert!(result.status.is_supported());
    }

    #[test]
    fn event_diagnostic_with_invalid_range_is_dropped_and_partial() {
        let line = envelope_with_events(&serde_json::json!([
            {
                "eventId": "e0",
                "state": {"status": "supported"},
                "diagnostic": {
                    "range": {"start": 900, "end": 901},
                    "severity": "error",
                    "message": "outside the document"
                }
            }
        ]));
        let response = parse_line(&line, "r1", 1).expect("event response");
        let result = diagnostics(
            &response,
            &prepared_view(),
            &snapshot(),
            &[event_info("e0")],
        );
        assert!(result.diagnostics.is_empty());
        let CapabilityStatus::Partial { reason } = result.status else {
            panic!("expected partial");
        };
        assert_eq!(reason, "an event diagnostic range could not be mapped");
    }

    #[test]
    fn partial_event_state_forces_partial_status() {
        let line = envelope_with_events(&serde_json::json!([
            {
                "eventId": "e0",
                "state": {"status": "partial", "reason": "multiple registrations"}
            }
        ]));
        let response = parse_line(&line, "r1", 1).expect("event response");
        let result = diagnostics(
            &response,
            &prepared_view(),
            &snapshot(),
            &[event_info("e0")],
        );
        assert!(result.diagnostics.is_empty());
        let CapabilityStatus::Partial { reason } = result.status else {
            panic!("expected partial");
        };
        assert_eq!(reason, "multiple registrations");
    }

    #[test]
    fn event_state_accepts_a_bare_status_string() {
        let line = envelope_with_events(&serde_json::json!([
            {"eventId": "e0", "state": "partial"}
        ]));
        let response = parse_line(&line, "r1", 1).expect("event response");
        let result = diagnostics(
            &response,
            &prepared_view(),
            &snapshot(),
            &[event_info("e0")],
        );
        let CapabilityStatus::Partial { reason } = result.status else {
            panic!("expected partial");
        };
        assert_eq!(reason, "event handler analysis is partial");
    }

    #[test]
    fn unknown_event_result_id_forces_partial_and_is_ignored() {
        let line = envelope_with_events(&serde_json::json!([
            {
                "eventId": "e9",
                "state": {"status": "supported"},
                "diagnostic": {
                    "range": {"start": 3, "end": 8},
                    "severity": "error",
                    "message": "not requested"
                }
            }
        ]));
        let response = parse_line(&line, "r1", 1).expect("event response");
        let result = diagnostics(
            &response,
            &prepared_view(),
            &snapshot(),
            &[event_info("e0")],
        );
        assert!(result.diagnostics.is_empty());
        let CapabilityStatus::Partial { reason } = result.status else {
            panic!("expected partial");
        };
        assert_eq!(reason, UNKNOWN_EVENT_RESULT_REASON);
    }

    #[test]
    fn oversized_event_result_array_is_truncated() {
        let entries = (0..=MAX_EVENT_RESULTS)
            .map(|_| serde_json::json!({"eventId": "e0", "state": {"status": "supported"}}))
            .collect::<Vec<_>>();
        let line = envelope_with_events(&serde_json::Value::Array(entries));
        let response = parse_line(&line, "r1", 1).expect("truncated event results");
        assert!(response.events_truncated);
        assert_eq!(response.event_results.len(), MAX_EVENT_RESULTS);
        let result = diagnostics(
            &response,
            &prepared_view(),
            &snapshot(),
            &[event_info("e0")],
        );
        let CapabilityStatus::Partial { reason } = result.status else {
            panic!("expected partial");
        };
        assert_eq!(reason, EVENT_RESULTS_TRUNCATED_REASON);
    }

    #[test]
    fn error_severity_is_default_for_event_diagnostics() {
        let line = envelope_with_events(&serde_json::json!([
            {
                "eventId": "e0",
                "diagnostic": {"range": {"start": 3, "end": 8}, "message": "no severity"}
            }
        ]));
        let response = parse_line(&line, "r1", 1).expect("event response");
        let result = diagnostics(
            &response,
            &prepared_view(),
            &snapshot(),
            &[event_info("e0")],
        );
        assert_eq!(result.diagnostics.len(), 1);
        assert_eq!(result.diagnostics[0].severity, DiagnosticSeverity::Error);
    }

    #[test]
    fn event_diagnostic_outside_handler_range_is_clamped_and_partial() {
        let line = envelope_with_events(&serde_json::json!([
            {
                "eventId": "e0",
                "state": {"status": "supported"},
                "diagnostic": {
                    "range": {"start": 0, "end": 1},
                    "severity": "error",
                    "message": "handler is missing"
                }
            }
        ]));
        let response = parse_line(&line, "r1", 1).expect("event response");
        let result = diagnostics(
            &response,
            &prepared_view(),
            &snapshot(),
            &[event_info("e0")],
        );
        assert_eq!(result.diagnostics.len(), 1);
        assert_eq!(
            result.diagnostics[0].range,
            SourceRange::from_bounds(2, 12).expect("clamped range")
        );
        let CapabilityStatus::Partial { reason } = result.status else {
            panic!("expected partial");
        };
        assert_eq!(reason, "an event diagnostic range could not be mapped");
    }

    #[test]
    fn missing_event_result_for_a_requested_event_forces_partial() {
        let line = envelope_with_events(&serde_json::json!([
            {"eventId": "e0", "state": {"status": "supported"}}
        ]));
        let response = parse_line(&line, "r1", 1).expect("event response");
        let result = diagnostics(
            &response,
            &prepared_view(),
            &snapshot(),
            &[event_info("e0"), event_info("e1")],
        );
        assert!(result.diagnostics.is_empty());
        let CapabilityStatus::Partial { reason } = result.status else {
            panic!("expected partial");
        };
        assert_eq!(reason, MISSING_EVENT_RESULT_REASON);
    }

    #[test]
    fn event_hover_reports_the_signature_at_the_handler_range() {
        let line = envelope_with_events(&serde_json::json!([
            {
                "eventId": "e0",
                "state": {"status": "supported"},
                "signature": "onTap(event: { detail: { x: number } })"
            }
        ]));
        let response = parse_line(&line, "r1", 1).expect("event response");
        let result = hover(
            &response,
            &prepared_view(),
            &BTreeMap::new(),
            &[event_info("e0")],
            4,
        );
        assert!(result.status.is_supported());
        assert_eq!(result.quality, TypeQuality::Known);
        assert_eq!(
            result.range,
            Some(SourceRange::from_bounds(2, 12).expect("handler range"))
        );
        assert!(
            result
                .contents
                .contains("onTap(event: { detail: { x: number } })")
        );
    }

    #[test]
    fn event_hover_without_a_result_forces_partial() {
        let line = envelope_with_events(&serde_json::json!([]));
        let response = parse_line(&line, "r1", 1).expect("event response");
        let result = hover(
            &response,
            &prepared_view(),
            &BTreeMap::new(),
            &[event_info("e0")],
            4,
        );
        assert!(result.contents.is_empty());
        assert_eq!(result.range, None);
        let CapabilityStatus::Partial { reason } = result.status else {
            panic!("expected partial");
        };
        assert_eq!(reason, MISSING_EVENT_RESULT_REASON);
    }
}
