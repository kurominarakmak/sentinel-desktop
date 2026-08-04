//! Phase 3C-B2-A private contract types and pure accounting only.
//!
//! B2-B0 remains the mandatory grammar-review gate. This module deliberately
//! contains no Git invocation, parser, persistence access, or bridge surface.

use sentinel_core::{ProjectId, WorktreeId};
use sentinel_git::{RepositoryMode, MAX_REPOSITORY_RELATIVE_PATH_BYTES};
use std::collections::BTreeMap;
use std::mem::size_of;
use std::sync::Arc;
use std::time::Duration;
use unicode_normalization::UnicodeNormalization;

const MIB: usize = 1024 * 1024;
const KIB: usize = 1024;

const MAX_CHILD_STDOUT_BYTES: usize = 2 * MIB;
const MAX_CHILD_STDERR_BYTES: usize = 16 * KIB;
const MAX_TOTAL_GIT_CHILDREN_PER_REQUEST: u16 = 88;
const MAX_TOTAL_CAPTURED_STDOUT_BYTES: usize = 8 * MIB;
const MAX_TOTAL_CAPTURED_STDERR_BYTES: usize = MIB;
const MAX_TEXT_BYTES_PER_EXTRACTION: usize = MIB;
const MAX_TOTAL_PARSED_DIFF_BYTES: usize = 3 * MIB;
const MAX_TOTAL_RETAINED_EXTRACTION_BYTES: usize = 3 * MIB;
const MAX_HUNKS_PER_SURFACE: usize = 512;
const MAX_TOTAL_HUNKS_PER_REQUEST: usize = 3_072;
const MAX_LINES_PER_SURFACE: usize = 20_000;
const MAX_TOTAL_LINES_PER_REQUEST: usize = 120_000;
const MAX_DIFF_LINE_BYTES: usize = 16 * KIB;
const MAX_SEMANTIC_ATTRIBUTE_VALUE_BYTES: usize = 4 * KIB;
const MAX_SELECTED_PATH_BYTES: usize = MAX_REPOSITORY_RELATIVE_PATH_BYTES;
const B2_OPERATION_DEADLINE: Duration = Duration::from_secs(15);
const MAX_RETAINED_EXTRACTION_STRUCTURES: u8 = 2;
const MAX_NON_EXTRACTABLE_REASONS_PER_SURFACE: usize = 16;

#[derive(Clone, Debug, PartialEq, Eq)]
struct BoundedTextualExtractionRequest {
    project_id: ProjectId,
    worktree_id: WorktreeId,
    selection: NonAuthoritativePathSelection,
}

impl BoundedTextualExtractionRequest {
    fn new(
        project_id: ProjectId,
        worktree_id: WorktreeId,
        selection: NonAuthoritativePathSelection,
    ) -> Self {
        Self {
            project_id,
            worktree_id,
            selection,
        }
    }
}

/// Pure structural binding only. B2-C must construct this after fresh
/// authoritative persistence and inventory resolution; B2-A performs no such
/// runtime validation itself.
#[derive(Clone, Debug, PartialEq, Eq)]
struct ResolvedTextualExtractionIdentity {
    project_id: ProjectId,
    worktree_id: WorktreeId,
    requested_selection: NonAuthoritativePathSelection,
    authoritative_path_identity: ValidatedInternalPathIdentity,
}

/// Pure identity binding only. B2-C remains responsible for proving that the
/// resolved identity came from fresh authoritative state before B2-A binds its
/// single request-wide path budget.
#[derive(Clone, Debug, PartialEq, Eq)]
struct ValidatedPathBudgetBinding {
    project_id: ProjectId,
    worktree_id: WorktreeId,
    authoritative_path_identity: ValidatedInternalPathIdentity,
    path_bytes: usize,
}

fn bind_validated_path_budget(
    request: &BoundedTextualExtractionRequest,
    resolved: &ResolvedTextualExtractionIdentity,
    evidence: &TextualExtractionEvidence,
) -> Result<ValidatedPathBudgetBinding, B2Error> {
    if request.project_id != resolved.project_id
        || request.worktree_id != resolved.worktree_id
        || request.selection != resolved.requested_selection
        || evidence.path != resolved.authoritative_path_identity
    {
        return Err(B2Error::InvalidRequest);
    }
    let path_bytes = resolved.authoritative_path_identity.0.len();
    if path_bytes == 0 || path_bytes > MAX_SELECTED_PATH_BYTES {
        return Err(B2Error::InvalidEvidence);
    }
    Ok(ValidatedPathBudgetBinding {
        project_id: request.project_id.clone(),
        worktree_id: request.worktree_id.clone(),
        authoritative_path_identity: resolved.authoritative_path_identity.clone(),
        path_bytes,
    })
}

impl ResolvedTextualExtractionIdentity {
    fn new(
        project_id: ProjectId,
        worktree_id: WorktreeId,
        requested_selection: NonAuthoritativePathSelection,
        authoritative_path_identity: ValidatedInternalPathIdentity,
    ) -> Self {
        Self {
            project_id,
            worktree_id,
            requested_selection,
            authoritative_path_identity,
        }
    }
}

/// A lexical candidate only. B2-C must match it against fresh authoritative
/// inventory for the request worktree before it becomes an internal identity.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct NonAuthoritativePathSelection(String);

impl NonAuthoritativePathSelection {
    fn new(value: String) -> Result<Self, B2Error> {
        validate_candidate_path(&value)?;
        Ok(Self(value))
    }

    fn as_str(&self) -> &str {
        &self.0
    }
}

fn validate_candidate_path(value: &str) -> Result<(), B2Error> {
    if value.is_empty()
        || value.len() > MAX_SELECTED_PATH_BYTES
        || value.contains('\0')
        || value.starts_with('/')
        || value.starts_with('\\')
        || value
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
        || (value.len() >= 2
            && value.as_bytes()[0].is_ascii_alphabetic()
            && value.as_bytes()[1] == b':')
    {
        return Err(B2Error::InvalidRequest);
    }
    // Do not permit platform Unicode normalization to collapse two exact Git
    // byte names into one selection identity.
    if value.nfc().collect::<String>() != value.nfd().collect::<String>() {
        return Err(B2Error::InvalidRequest);
    }
    #[cfg(windows)]
    if value.contains('\\') {
        return Err(B2Error::InvalidRequest);
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct ValidatedInternalPathIdentity(String);

impl ValidatedInternalPathIdentity {
    fn from_authoritative_path(value: String) -> Result<Self, B2Error> {
        validate_candidate_path(&value)?;
        Ok(Self(value))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TextualExtractionEvidence {
    path: ValidatedInternalPathIdentity,
    staged: SurfaceExtractionState,
    unstaged: SurfaceExtractionState,
    attributes: Vec<AttributeEligibilityEvidence>,
}

impl TextualExtractionEvidence {
    fn validate(&self) -> Result<(), B2Error> {
        validate_attributes(&self.attributes)?;
        validate_surface(&self.staged)?;
        validate_surface(&self.unstaged)?;
        validate_extractable_attribute_policy(&self.attributes, &self.staged, &self.unstaged)?;
        Ok(())
    }

    fn is_no_textual_surface(&self) -> bool {
        !matches!(self.staged, SurfaceExtractionState::Extractable(_))
            && !matches!(self.unstaged, SurfaceExtractionState::Extractable(_))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum SurfaceExtractionState {
    Absent,
    NonExtractable(NonExtractableReasons),
    Extractable(TextualSurfaceEvidence),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum NonExtractableReason {
    Binary,
    ModeOnly,
    Symlink,
    Untracked,
    Conflict,
    FilterConfigured,
    FilterDeferred,
    DiffAttributeConfigured,
    TextAttributeConfigured,
    BinaryAttributeConfigured,
    EolAttributeConfigured,
    WorkingTreeEncodingConfigured,
    Gitlink,
    SparseOrSkipWorktree,
    UnsupportedType,
    ClassificationUnavailable,
    AttributeUnsupported,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct NonExtractableReasons(Vec<NonExtractableReason>);

impl NonExtractableReasons {
    fn new(mut reasons: Vec<NonExtractableReason>) -> Result<Self, B2Error> {
        if reasons.is_empty() || reasons.len() > MAX_NON_EXTRACTABLE_REASONS_PER_SURFACE {
            return Err(B2Error::InvalidEvidence);
        }
        reasons.sort_unstable();
        if reasons.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(B2Error::InvalidEvidence);
        }
        Ok(Self(reasons))
    }

    fn contains(&self, reason: NonExtractableReason) -> bool {
        self.0.binary_search(&reason).is_ok()
    }

    fn iter(&self) -> impl Iterator<Item = NonExtractableReason> + '_ {
        self.0.iter().copied()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TextualSurfaceEvidence {
    old_mode: Option<RepositoryMode>,
    new_mode: Option<RepositoryMode>,
    hunks: Vec<TextualHunk>,
    total_additions: u64,
    total_deletions: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TextualHunk {
    old_start: u64,
    old_count: u64,
    new_start: u64,
    new_count: u64,
    lines: Vec<TextualDiffLine>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TextualDiffLine {
    kind: TextualDiffLineKind,
    text: String,
    old_line_ending: Option<LineEnding>,
    new_line_ending: Option<LineEnding>,
    old_no_newline_marker: bool,
    new_no_newline_marker: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TextualDiffLineKind {
    Context,
    Addition,
    Deletion,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LineEnding {
    Lf,
    CrLf,
    None,
}

fn validate_surface(surface: &SurfaceExtractionState) -> Result<(), B2Error> {
    if let SurfaceExtractionState::Extractable(evidence) = surface {
        if evidence.hunks.len() > MAX_HUNKS_PER_SURFACE {
            return Err(B2Error::InvalidEvidence);
        }
        let mut additions = 0_u64;
        let mut deletions = 0_u64;
        let mut lines = 0_usize;
        let mut previous = None;
        let mut old_final_line_seen = false;
        let mut new_final_line_seen = false;
        for hunk in &evidence.hunks {
            let coordinate = (hunk.old_start, hunk.new_start);
            if previous.is_some_and(|prior| coordinate < prior) {
                return Err(B2Error::InvalidEvidence);
            }
            previous = Some(coordinate);
            let mut old_lines = 0_u64;
            let mut new_lines = 0_u64;
            for line in &hunk.lines {
                lines = lines.checked_add(1).ok_or(B2Error::InvalidEvidence)?;
                let old_applicable = matches!(
                    line.kind,
                    TextualDiffLineKind::Context | TextualDiffLineKind::Deletion
                );
                let new_applicable = matches!(
                    line.kind,
                    TextualDiffLineKind::Context | TextualDiffLineKind::Addition
                );
                if (old_final_line_seen && old_applicable)
                    || (new_final_line_seen && new_applicable)
                {
                    return Err(B2Error::InvalidEvidence);
                }
                validate_line(line)?;
                old_final_line_seen |= matches!(line.old_line_ending, Some(LineEnding::None));
                new_final_line_seen |= matches!(line.new_line_ending, Some(LineEnding::None));
                match line.kind {
                    TextualDiffLineKind::Context => {
                        old_lines = old_lines.checked_add(1).ok_or(B2Error::InvalidEvidence)?;
                        new_lines = new_lines.checked_add(1).ok_or(B2Error::InvalidEvidence)?;
                    }
                    TextualDiffLineKind::Addition => {
                        additions = additions.checked_add(1).ok_or(B2Error::InvalidEvidence)?;
                        new_lines = new_lines.checked_add(1).ok_or(B2Error::InvalidEvidence)?;
                    }
                    TextualDiffLineKind::Deletion => {
                        deletions = deletions.checked_add(1).ok_or(B2Error::InvalidEvidence)?;
                        old_lines = old_lines.checked_add(1).ok_or(B2Error::InvalidEvidence)?;
                    }
                }
            }
            if old_lines != hunk.old_count || new_lines != hunk.new_count {
                return Err(B2Error::InvalidEvidence);
            }
        }
        if lines > MAX_LINES_PER_SURFACE
            || additions != evidence.total_additions
            || deletions != evidence.total_deletions
        {
            return Err(B2Error::InvalidEvidence);
        }
    }
    Ok(())
}

fn validate_line(line: &TextualDiffLine) -> Result<(), B2Error> {
    if line.text.len() > MAX_DIFF_LINE_BYTES
        || line.text.chars().any(|value| {
            value == '\u{1b}'
                || (value.is_control() && value != '\t')
                || matches!(value, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        })
    {
        return Err(B2Error::InvalidEvidence);
    }
    let sides = match line.kind {
        TextualDiffLineKind::Context => (true, true),
        TextualDiffLineKind::Addition => (false, true),
        TextualDiffLineKind::Deletion => (true, false),
    };
    validate_line_side(line.old_line_ending, line.old_no_newline_marker, sides.0)?;
    validate_line_side(line.new_line_ending, line.new_no_newline_marker, sides.1)?;
    Ok(())
}

fn validate_line_side(
    ending: Option<LineEnding>,
    marker: bool,
    expected: bool,
) -> Result<(), B2Error> {
    if expected != ending.is_some() {
        return Err(B2Error::InvalidEvidence);
    }
    if marker != matches!(ending, Some(LineEnding::None)) {
        return Err(B2Error::InvalidEvidence);
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum GitSemanticAttribute {
    Filter,
    Diff,
    Text,
    Binary,
    Eol,
    WorkingTreeEncoding,
}

impl GitSemanticAttribute {
    const REQUIRED: [Self; 6] = [
        Self::Filter,
        Self::Diff,
        Self::Text,
        Self::Binary,
        Self::Eol,
        Self::WorkingTreeEncoding,
    ];
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum GitAttributeState {
    Unspecified,
    Set,
    Unset,
    Value(String),
    /// A bounded protocol-level filter deferral, never a magic attribute value.
    Deferred,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AttributeEligibilityDecision {
    Eligible,
    NonExtractable(AttributeIneligibilityReason),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AttributeIneligibilityReason {
    Set,
    Unset,
    Value,
    Deferred,
    Unsupported,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct AttributeEligibilityEvidence {
    attribute: GitSemanticAttribute,
    state: GitAttributeState,
    decision: AttributeEligibilityDecision,
}

impl AttributeEligibilityEvidence {
    fn new(attribute: GitSemanticAttribute, state: GitAttributeState) -> Result<Self, B2Error> {
        if matches!(&state, GitAttributeState::Value(value) if value.len() > MAX_SEMANTIC_ATTRIBUTE_VALUE_BYTES)
        {
            return Err(B2Error::InvalidEvidence);
        }
        let decision = match state {
            GitAttributeState::Unspecified => AttributeEligibilityDecision::Eligible,
            GitAttributeState::Set => {
                AttributeEligibilityDecision::NonExtractable(AttributeIneligibilityReason::Set)
            }
            GitAttributeState::Unset => {
                AttributeEligibilityDecision::NonExtractable(AttributeIneligibilityReason::Unset)
            }
            GitAttributeState::Value(_) => {
                AttributeEligibilityDecision::NonExtractable(AttributeIneligibilityReason::Value)
            }
            GitAttributeState::Deferred if attribute == GitSemanticAttribute::Filter => {
                AttributeEligibilityDecision::NonExtractable(AttributeIneligibilityReason::Deferred)
            }
            GitAttributeState::Deferred => return Err(B2Error::InvalidEvidence),
        };
        Ok(Self {
            attribute,
            state,
            decision,
        })
    }
}

fn validate_attributes(attributes: &[AttributeEligibilityEvidence]) -> Result<(), B2Error> {
    if attributes.len() != GitSemanticAttribute::REQUIRED.len() {
        return Err(B2Error::InvalidEvidence);
    }
    for (expected, evidence) in GitSemanticAttribute::REQUIRED.iter().zip(attributes) {
        if expected != &evidence.attribute {
            return Err(B2Error::InvalidEvidence);
        }
        let rebuilt =
            AttributeEligibilityEvidence::new(evidence.attribute, evidence.state.clone())?;
        if rebuilt.decision != evidence.decision {
            return Err(B2Error::InvalidEvidence);
        }
    }
    Ok(())
}

fn validate_extractable_attribute_policy(
    attributes: &[AttributeEligibilityEvidence],
    staged: &SurfaceExtractionState,
    unstaged: &SurfaceExtractionState,
) -> Result<(), B2Error> {
    let attribute_reasons = required_attribute_reasons(attributes)?;
    let has_extractable_surface = matches!(staged, SurfaceExtractionState::Extractable(_))
        || matches!(unstaged, SurfaceExtractionState::Extractable(_));
    if has_extractable_surface && !attribute_reasons.is_empty() {
        return Err(B2Error::InvalidEvidence);
    }
    for surface in [staged, unstaged] {
        if let SurfaceExtractionState::NonExtractable(reasons) = surface {
            for reason in &attribute_reasons {
                if !reasons.contains(*reason) {
                    return Err(B2Error::InvalidEvidence);
                }
            }
            for reason in reasons.iter().filter(|reason| reason.is_attribute_reason()) {
                if !attribute_reasons.contains(&reason) {
                    return Err(B2Error::InvalidEvidence);
                }
            }
        }
    }
    Ok(())
}

impl NonExtractableReason {
    fn is_attribute_reason(self) -> bool {
        matches!(
            self,
            Self::FilterConfigured
                | Self::FilterDeferred
                | Self::DiffAttributeConfigured
                | Self::TextAttributeConfigured
                | Self::BinaryAttributeConfigured
                | Self::EolAttributeConfigured
                | Self::WorkingTreeEncodingConfigured
        )
    }
}

fn required_attribute_reasons(
    attributes: &[AttributeEligibilityEvidence],
) -> Result<Vec<NonExtractableReason>, B2Error> {
    let mut reasons = Vec::new();
    for attribute in attributes {
        if attribute.decision == AttributeEligibilityDecision::Eligible {
            continue;
        }
        let reason = match (&attribute.attribute, &attribute.state) {
            (GitSemanticAttribute::Filter, GitAttributeState::Deferred) => {
                NonExtractableReason::FilterDeferred
            }
            (GitSemanticAttribute::Filter, _) => NonExtractableReason::FilterConfigured,
            (GitSemanticAttribute::Diff, _) => NonExtractableReason::DiffAttributeConfigured,
            (GitSemanticAttribute::Text, _) => NonExtractableReason::TextAttributeConfigured,
            (GitSemanticAttribute::Binary, _) => NonExtractableReason::BinaryAttributeConfigured,
            (GitSemanticAttribute::Eol, _) => NonExtractableReason::EolAttributeConfigured,
            (GitSemanticAttribute::WorkingTreeEncoding, _) => {
                NonExtractableReason::WorkingTreeEncodingConfigured
            }
        };
        reasons.push(reason);
    }
    reasons.sort_unstable();
    if reasons.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(B2Error::InvalidEvidence);
    }
    Ok(reasons)
}

/// A conservative normalized-retention accounting unit, not a claim about
/// allocator-exact memory. It counts every normalized node plus owned bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RetainedEvidenceCharge(usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct MeasuredEvidenceMetrics {
    retained_charge: RetainedEvidenceCharge,
    normalized_text_bytes: usize,
    line_count: usize,
    hunk_count: usize,
    path_bytes: usize,
}

/// Owns structurally validated evidence so its measured charge cannot be
/// detached from or silently reused for mutable evidence.
#[derive(Clone, Debug, PartialEq, Eq)]
struct ValidatedMeasuredEvidence {
    evidence: TextualExtractionEvidence,
    retained_charge: RetainedEvidenceCharge,
    normalized_text_bytes: usize,
    line_count: usize,
    hunk_count: usize,
    path_bytes: usize,
}

impl ValidatedMeasuredEvidence {
    fn new(evidence: TextualExtractionEvidence) -> Result<Self, B2Error> {
        evidence.validate()?;
        let metrics = measure_evidence(&evidence)?;
        if metrics.retained_charge.0 == 0 {
            return Err(B2Error::InvalidEvidence);
        }
        Ok(Self {
            evidence,
            retained_charge: metrics.retained_charge,
            normalized_text_bytes: metrics.normalized_text_bytes,
            line_count: metrics.line_count,
            hunk_count: metrics.hunk_count,
            path_bytes: metrics.path_bytes,
        })
    }
}

fn measure_evidence(
    evidence: &TextualExtractionEvidence,
) -> Result<MeasuredEvidenceMetrics, B2Error> {
    let mut total = size_of::<TextualExtractionEvidence>();
    let path_bytes = evidence.path.0.len();
    add_retained_bytes(&mut total, path_bytes)?;
    let mut normalized_text_bytes = 0_usize;
    let mut line_count = 0_usize;
    let mut hunk_count = 0_usize;
    for attribute in &evidence.attributes {
        add_retained_bytes(&mut total, size_of::<AttributeEligibilityEvidence>())?;
        if let GitAttributeState::Value(value) = &attribute.state {
            add_retained_bytes(&mut total, value.len())?;
        }
    }
    for surface in [&evidence.staged, &evidence.unstaged] {
        match surface {
            SurfaceExtractionState::Absent => {}
            SurfaceExtractionState::NonExtractable(reasons) => {
                add_retained_bytes(
                    &mut total,
                    reasons
                        .0
                        .len()
                        .checked_mul(size_of::<NonExtractableReason>())
                        .ok_or(B2Error::InvalidEvidence)?,
                )?;
            }
            SurfaceExtractionState::Extractable(surface) => {
                add_retained_bytes(&mut total, size_of::<TextualSurfaceEvidence>())?;
                for hunk in &surface.hunks {
                    hunk_count = hunk_count.checked_add(1).ok_or(B2Error::InvalidEvidence)?;
                    add_retained_bytes(&mut total, size_of::<TextualHunk>())?;
                    for line in &hunk.lines {
                        line_count = line_count.checked_add(1).ok_or(B2Error::InvalidEvidence)?;
                        normalized_text_bytes = normalized_text_bytes
                            .checked_add(line.text.len())
                            .ok_or(B2Error::InvalidEvidence)?;
                        add_retained_bytes(&mut total, size_of::<TextualDiffLine>())?;
                        add_retained_bytes(&mut total, line.text.len())?;
                    }
                }
            }
        }
    }
    Ok(MeasuredEvidenceMetrics {
        retained_charge: RetainedEvidenceCharge(total),
        normalized_text_bytes,
        line_count,
        hunk_count,
        path_bytes,
    })
}

fn add_retained_bytes(total: &mut usize, bytes: usize) -> Result<(), B2Error> {
    *total = total.checked_add(bytes).ok_or(B2Error::InvalidEvidence)?;
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum B2BudgetLimit {
    Children,
    CapturedStdout,
    CapturedStderr,
    ParsedDiff,
    NormalizedText,
    RetainedEvidence,
    RetainedStructures,
    Lines,
    Hunks,
    Paths,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum B2BudgetError {
    Limit(B2BudgetLimit),
    ArithmeticOverflow,
    ReleaseUnderflow,
    PathBindingMismatch,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
struct B2BudgetUsage {
    children: u16,
    captured_stdout: usize,
    captured_stderr: usize,
    parsed_diff: usize,
    normalized_text: usize,
    retained_extraction: usize,
    retained_structures: u8,
    lines: usize,
    hunks: usize,
    paths: usize,
}

#[derive(Debug)]
struct B2RequestBudget {
    usage: B2BudgetUsage,
    bound_path: Option<ValidatedPathBudgetBinding>,
    ledger: Arc<RetentionLedgerIdentity>,
    active_retentions: BTreeMap<u64, usize>,
    next_retention_id: u64,
}

#[derive(Debug)]
struct RetentionLedgerIdentity;

/// Deliberately non-Clone and non-Copy: only this budget can consume a
/// reservation, and it releases the ledger-recorded exact allocation.
#[derive(Debug)]
struct RetentionToken {
    ledger: Arc<RetentionLedgerIdentity>,
    allocation_id: u64,
}

impl Default for B2RequestBudget {
    fn default() -> Self {
        Self {
            usage: B2BudgetUsage::default(),
            bound_path: None,
            ledger: Arc::new(RetentionLedgerIdentity),
            active_retentions: BTreeMap::new(),
            next_retention_id: 0,
        }
    }
}

impl B2RequestBudget {
    fn charge_child(&mut self, stdout: usize, stderr: usize) -> Result<(), B2BudgetError> {
        let children = checked_add_u16(
            self.usage.children,
            1,
            MAX_TOTAL_GIT_CHILDREN_PER_REQUEST,
            B2BudgetLimit::Children,
        )?;
        let captured_stdout = checked_add_usize(
            self.usage.captured_stdout,
            stdout,
            MAX_TOTAL_CAPTURED_STDOUT_BYTES,
            B2BudgetLimit::CapturedStdout,
        )?;
        let captured_stderr = checked_add_usize(
            self.usage.captured_stderr,
            stderr,
            MAX_TOTAL_CAPTURED_STDERR_BYTES,
            B2BudgetLimit::CapturedStderr,
        )?;
        if stdout > MAX_CHILD_STDOUT_BYTES {
            return Err(B2BudgetError::Limit(B2BudgetLimit::CapturedStdout));
        }
        if stderr > MAX_CHILD_STDERR_BYTES {
            return Err(B2BudgetError::Limit(B2BudgetLimit::CapturedStderr));
        }
        self.usage.children = children;
        self.usage.captured_stdout = captured_stdout;
        self.usage.captured_stderr = captured_stderr;
        Ok(())
    }

    fn reserve_measured_evidence(
        &mut self,
        path_binding: &ValidatedPathBudgetBinding,
        measured: &ValidatedMeasuredEvidence,
        parsed_diff: usize,
    ) -> Result<RetentionToken, B2BudgetError> {
        if measured.evidence.path != path_binding.authoritative_path_identity
            || measured.path_bytes != path_binding.path_bytes
        {
            return Err(B2BudgetError::PathBindingMismatch);
        }
        let (bind_path, path_total) = self.prospective_path_binding(path_binding)?;
        let retained = measured.retained_charge.0;
        if measured.normalized_text_bytes > MAX_TEXT_BYTES_PER_EXTRACTION {
            return Err(B2BudgetError::Limit(B2BudgetLimit::NormalizedText));
        }
        let parsed_total = checked_add_usize(
            self.usage.parsed_diff,
            parsed_diff,
            MAX_TOTAL_PARSED_DIFF_BYTES,
            B2BudgetLimit::ParsedDiff,
        )?;
        let text_total = checked_add_usize(
            self.usage.normalized_text,
            measured.normalized_text_bytes,
            MAX_TOTAL_PARSED_DIFF_BYTES,
            B2BudgetLimit::NormalizedText,
        )?;
        let retained_total = checked_add_usize(
            self.usage.retained_extraction,
            retained,
            MAX_TOTAL_RETAINED_EXTRACTION_BYTES,
            B2BudgetLimit::RetainedEvidence,
        )?;
        let retained_structures = checked_add_u8(
            u8::try_from(self.active_retentions.len())
                .map_err(|_| B2BudgetError::ArithmeticOverflow)?,
            1,
            MAX_RETAINED_EXTRACTION_STRUCTURES,
            B2BudgetLimit::RetainedStructures,
        )?;
        let line_total = checked_add_usize(
            self.usage.lines,
            measured.line_count,
            MAX_TOTAL_LINES_PER_REQUEST,
            B2BudgetLimit::Lines,
        )?;
        let hunk_total = checked_add_usize(
            self.usage.hunks,
            measured.hunk_count,
            MAX_TOTAL_HUNKS_PER_REQUEST,
            B2BudgetLimit::Hunks,
        )?;
        let allocation_id = self
            .next_retention_id
            .checked_add(1)
            .ok_or(B2BudgetError::ArithmeticOverflow)?;
        if bind_path {
            self.bound_path = Some(path_binding.clone());
        }
        self.usage.paths = path_total;
        self.usage.parsed_diff = parsed_total;
        self.usage.normalized_text = text_total;
        self.usage.retained_extraction = retained_total;
        self.usage.retained_structures = retained_structures;
        self.usage.lines = line_total;
        self.usage.hunks = hunk_total;
        self.active_retentions.insert(allocation_id, retained);
        self.next_retention_id = allocation_id;
        Ok(RetentionToken {
            ledger: Arc::clone(&self.ledger),
            allocation_id,
        })
    }

    fn bind_accepted_outcome(&mut self, outcome: &B2AcceptedResult) -> Result<(), B2BudgetError> {
        let binding = outcome.path_binding();
        let (bind_path, path_total) = self.prospective_path_binding(binding)?;
        if bind_path {
            self.bound_path = Some(binding.clone());
        }
        self.usage.paths = path_total;
        Ok(())
    }

    fn prospective_path_binding(
        &self,
        binding: &ValidatedPathBudgetBinding,
    ) -> Result<(bool, usize), B2BudgetError> {
        match &self.bound_path {
            Some(existing) if existing == binding => Ok((false, self.usage.paths)),
            Some(_) => Err(B2BudgetError::PathBindingMismatch),
            None => Ok((
                true,
                checked_add_usize(
                    self.usage.paths,
                    binding.path_bytes,
                    MAX_SELECTED_PATH_BYTES,
                    B2BudgetLimit::Paths,
                )?,
            )),
        }
    }

    fn release_retained(&mut self, token: RetentionToken) -> Result<(), B2BudgetError> {
        if !Arc::ptr_eq(&self.ledger, &token.ledger) {
            return Err(B2BudgetError::ReleaseUnderflow);
        }
        let Some(bytes) = self.active_retentions.get(&token.allocation_id).copied() else {
            return Err(B2BudgetError::ReleaseUnderflow);
        };
        let retained_extraction = self
            .usage
            .retained_extraction
            .checked_sub(bytes)
            .ok_or(B2BudgetError::ReleaseUnderflow)?;
        let retained_structures = self
            .usage
            .retained_structures
            .checked_sub(1)
            .ok_or(B2BudgetError::ReleaseUnderflow)?;
        self.active_retentions.remove(&token.allocation_id);
        self.usage.retained_extraction = retained_extraction;
        self.usage.retained_structures = retained_structures;
        Ok(())
    }
}

fn checked_add_usize(
    current: usize,
    added: usize,
    limit: usize,
    kind: B2BudgetLimit,
) -> Result<usize, B2BudgetError> {
    let total = current
        .checked_add(added)
        .ok_or(B2BudgetError::ArithmeticOverflow)?;
    if total > limit {
        return Err(B2BudgetError::Limit(kind));
    }
    Ok(total)
}

fn checked_add_u16(
    current: u16,
    added: u16,
    limit: u16,
    kind: B2BudgetLimit,
) -> Result<u16, B2BudgetError> {
    let total = current
        .checked_add(added)
        .ok_or(B2BudgetError::ArithmeticOverflow)?;
    if total > limit {
        return Err(B2BudgetError::Limit(kind));
    }
    Ok(total)
}

fn checked_add_u8(
    current: u8,
    added: u8,
    limit: u8,
    kind: B2BudgetLimit,
) -> Result<u8, B2BudgetError> {
    let total = current
        .checked_add(added)
        .ok_or(B2BudgetError::ArithmeticOverflow)?;
    if total > limit {
        return Err(B2BudgetError::Limit(kind));
    }
    Ok(total)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum B2Error {
    ChangeStale,
    WorktreeNotReady,
    ProjectUnavailable,
    WorktreeUnavailable,
    InventoryUnavailable,
    ClassificationUnavailable,
    NotTextEligible,
    MalformedOutput,
    PathMismatch,
    OutputOverflow,
    ResourceLimit,
    Timeout,
    GitCommandFailed,
    OperationCancelled,
    InvalidRequest,
    InvalidEvidence,
}

impl B2Error {
    pub(crate) fn safe_code(&self) -> &'static str {
        match self {
            Self::ChangeStale => "change_stale",
            Self::WorktreeNotReady => "worktree_not_ready",
            Self::ProjectUnavailable | Self::WorktreeUnavailable => "selection_unavailable",
            Self::InventoryUnavailable | Self::ClassificationUnavailable => "change_unavailable",
            Self::NotTextEligible => "not_text_eligible",
            Self::MalformedOutput => "diff_unavailable",
            Self::PathMismatch | Self::InvalidRequest => "invalid_selection",
            Self::OutputOverflow | Self::ResourceLimit => "resource_limit",
            Self::Timeout => "deadline_exceeded",
            Self::GitCommandFailed => "git_unavailable",
            Self::OperationCancelled => "operation_cancelled",
            Self::InvalidEvidence => "change_unavailable",
        }
    }
}

/// Redacted, path-free information which may cross the Phase 3C-C boundary.
#[derive(Clone, Debug, serde::Serialize, PartialEq, Eq)]
pub(crate) struct PublicTextualSummary {
    pub(crate) textual_available: bool,
    pub(crate) staged_additions: u64,
    pub(crate) staged_deletions: u64,
    pub(crate) unstaged_additions: u64,
    pub(crate) unstaged_deletions: u64,
    pub(crate) hunk_count: usize,
}

/// Strictly parse both raw Git envelopes into private B2-A evidence, then
/// return only an aggregate redacted summary. The caller owns fresh inventory,
/// classification and lifecycle coherence; this primitive performs no I/O.
pub(crate) fn parse_private_diff_pair(
    authoritative_path: String,
    staged: &[u8],
    unstaged: &[u8],
) -> Result<PublicTextualSummary, B2Error> {
    let path = ValidatedInternalPathIdentity::from_authoritative_path(authoritative_path)?;
    let expected_path = path.0.clone();
    let attributes = GitSemanticAttribute::REQUIRED
        .iter()
        .map(|attribute| {
            AttributeEligibilityEvidence::new(*attribute, GitAttributeState::Unspecified)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let evidence = TextualExtractionEvidence {
        path,
        staged: parse_unified_surface(staged, &expected_path)?,
        unstaged: parse_unified_surface(unstaged, &expected_path)?,
        attributes,
    };
    evidence.validate()?;
    let (
        mut staged_additions,
        mut staged_deletions,
        mut unstaged_additions,
        mut unstaged_deletions,
        mut hunk_count,
    ): (u64, u64, u64, u64, usize) = (0, 0, 0, 0, 0);
    for (surface, additions, deletions) in [
        (
            &evidence.staged,
            &mut staged_additions,
            &mut staged_deletions,
        ),
        (
            &evidence.unstaged,
            &mut unstaged_additions,
            &mut unstaged_deletions,
        ),
    ] {
        if let SurfaceExtractionState::Extractable(value) = surface {
            *additions = value.total_additions;
            *deletions = value.total_deletions;
            hunk_count = hunk_count
                .checked_add(value.hunks.len())
                .ok_or(B2Error::ResourceLimit)?;
        }
    }
    Ok(PublicTextualSummary {
        textual_available: matches!(evidence.staged, SurfaceExtractionState::Extractable(_))
            || matches!(evidence.unstaged, SurfaceExtractionState::Extractable(_)),
        staged_additions,
        staged_deletions,
        unstaged_additions,
        unstaged_deletions,
        hunk_count,
    })
}

fn parse_unified_surface(
    bytes: &[u8],
    expected_path: &str,
) -> Result<SurfaceExtractionState, B2Error> {
    if bytes.is_empty() {
        return Ok(SurfaceExtractionState::Absent);
    }
    if bytes.len() > MAX_TEXT_BYTES_PER_EXTRACTION {
        return Err(B2Error::OutputOverflow);
    }
    let text = std::str::from_utf8(bytes).map_err(|_| B2Error::MalformedOutput)?;
    if text.contains('\0')
        || text.chars().any(|ch| {
            ch == '\u{1b}' || matches!(ch, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        })
    {
        return Err(B2Error::MalformedOutput);
    }
    if text.contains("GIT binary patch") || text.contains("Binary files ") {
        return Ok(SurfaceExtractionState::NonExtractable(
            NonExtractableReasons::new(vec![NonExtractableReason::Binary])?,
        ));
    }
    let mut lines = text.split_inclusive('\n').peekable();
    let first = lines.next().ok_or(B2Error::MalformedOutput)?;
    if first.trim_end_matches(['\r', '\n'])
        != format!("diff --git a/{expected_path} b/{expected_path}")
        || text.matches("\ndiff --git ").count() != 0
    {
        return Err(B2Error::MalformedOutput);
    }
    if text.contains("diff --cc ")
        || text.contains("diff --combined ")
        || text.contains("rename from ")
        || text.contains("rename to ")
    {
        return Err(B2Error::MalformedOutput);
    }
    let mut old_header = false;
    let mut new_header = false;
    let mut current: Option<TextualHunk> = None;
    let mut hunks = Vec::new();
    for raw in lines {
        let (line, record_ending) = if let Some(line) = raw.strip_suffix("\r\n") {
            (line, LineEnding::CrLf)
        } else if let Some(line) = raw.strip_suffix('\n') {
            (line, LineEnding::Lf)
        } else {
            return Err(B2Error::MalformedOutput);
        };
        if line.starts_with("--- ") {
            old_header = true;
            continue;
        }
        if line.starts_with("+++ ") {
            new_header = true;
            continue;
        }
        if line.starts_with("@@ ") {
            if !old_header || !new_header {
                return Err(B2Error::MalformedOutput);
            }
            if let Some(hunk) = current.take() {
                hunks.push(hunk);
            }
            current = Some(parse_hunk_header(line)?);
            continue;
        }
        // Git's fixed envelope has metadata between `diff --git` and the
        // mandatory old/new headers. It is deliberately ignored here only as
        // framing, never interpreted as evidence.
        if current.is_none() && !old_header && !new_header {
            if line.starts_with("index ")
                || line.starts_with("old mode ")
                || line.starts_with("new mode ")
                || line.starts_with("new file mode ")
                || line.starts_with("deleted file mode ")
                || line.starts_with("similarity index ")
            {
                continue;
            }
            return Err(B2Error::MalformedOutput);
        }
        if line == "\\ No newline at end of file" {
            let hunk = current.as_mut().ok_or(B2Error::MalformedOutput)?;
            let previous = hunk.lines.last_mut().ok_or(B2Error::MalformedOutput)?;
            if previous.old_no_newline_marker || previous.new_no_newline_marker {
                return Err(B2Error::MalformedOutput);
            }
            match previous.kind {
                TextualDiffLineKind::Context => {
                    previous.old_line_ending = Some(LineEnding::None);
                    previous.new_line_ending = Some(LineEnding::None);
                    previous.old_no_newline_marker = true;
                    previous.new_no_newline_marker = true;
                }
                TextualDiffLineKind::Deletion => {
                    previous.old_line_ending = Some(LineEnding::None);
                    previous.old_no_newline_marker = true;
                }
                TextualDiffLineKind::Addition => {
                    previous.new_line_ending = Some(LineEnding::None);
                    previous.new_no_newline_marker = true;
                }
            }
            continue;
        }
        let hunk = current.as_mut().ok_or(B2Error::MalformedOutput)?;
        let (kind, payload) = match line.as_bytes().first() {
            Some(b' ') => (TextualDiffLineKind::Context, &line[1..]),
            Some(b'+') => (TextualDiffLineKind::Addition, &line[1..]),
            Some(b'-') => (TextualDiffLineKind::Deletion, &line[1..]),
            _ => return Err(B2Error::MalformedOutput),
        };
        let (old_line_ending, new_line_ending) = match kind {
            TextualDiffLineKind::Context => (Some(record_ending), Some(record_ending)),
            TextualDiffLineKind::Addition => (None, Some(record_ending)),
            TextualDiffLineKind::Deletion => (Some(record_ending), None),
        };
        hunk.lines.push(TextualDiffLine {
            kind,
            text: payload.to_owned(),
            old_line_ending,
            new_line_ending,
            old_no_newline_marker: false,
            new_no_newline_marker: false,
        });
    }
    if let Some(hunk) = current {
        hunks.push(hunk);
    }
    if !old_header || !new_header {
        return Err(B2Error::MalformedOutput);
    }
    if hunks.is_empty() {
        return Ok(SurfaceExtractionState::NonExtractable(
            NonExtractableReasons::new(vec![NonExtractableReason::ModeOnly])?,
        ));
    }
    let total_additions = hunks
        .iter()
        .flat_map(|h| &h.lines)
        .filter(|line| line.kind == TextualDiffLineKind::Addition)
        .count() as u64;
    let total_deletions = hunks
        .iter()
        .flat_map(|h| &h.lines)
        .filter(|line| line.kind == TextualDiffLineKind::Deletion)
        .count() as u64;
    Ok(SurfaceExtractionState::Extractable(
        TextualSurfaceEvidence {
            old_mode: None,
            new_mode: None,
            hunks,
            total_additions,
            total_deletions,
        },
    ))
}

fn parse_hunk_header(line: &str) -> Result<TextualHunk, B2Error> {
    let body = line
        .strip_prefix("@@ -")
        .and_then(|value| value.split_once(" @@").map(|(body, _)| body))
        .ok_or(B2Error::MalformedOutput)?;
    let (old, new) = body.split_once(" +").ok_or(B2Error::MalformedOutput)?;
    let (old_start, old_count) = parse_range(old)?;
    let (new_start, new_count) = parse_range(new)?;
    Ok(TextualHunk {
        old_start,
        old_count,
        new_start,
        new_count,
        lines: Vec::new(),
    })
}

fn parse_range(value: &str) -> Result<(u64, u64), B2Error> {
    let (start, count) = match value.split_once(',') {
        Some(pair) => pair,
        None => (value, "1"),
    };
    let start = start.parse().map_err(|_| B2Error::MalformedOutput)?;
    let count = count.parse().map_err(|_| B2Error::MalformedOutput)?;
    Ok((start, count))
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum B2AcceptedResult {
    Textual {
        evidence: ValidatedMeasuredEvidence,
        path_binding: ValidatedPathBudgetBinding,
    },
    NoTextualSurface {
        path_binding: ValidatedPathBudgetBinding,
    },
    NotTextEligible {
        summary: NonTextEligibilitySummary,
        path_binding: ValidatedPathBudgetBinding,
    },
}

impl B2AcceptedResult {
    fn path_binding(&self) -> &ValidatedPathBudgetBinding {
        match self {
            Self::Textual { path_binding, .. }
            | Self::NoTextualSurface { path_binding }
            | Self::NotTextEligible { path_binding, .. } => path_binding,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum NonTextEligibilitySummary {
    Staged(NonExtractableReasons),
    Unstaged(NonExtractableReasons),
    Both {
        staged: NonExtractableReasons,
        unstaged: NonExtractableReasons,
    },
}

fn accept_evidence(
    request: &BoundedTextualExtractionRequest,
    resolved: &ResolvedTextualExtractionIdentity,
    evidence: TextualExtractionEvidence,
) -> Result<B2AcceptedResult, B2Error> {
    let path_binding = bind_validated_path_budget(request, resolved, &evidence)?;
    evidence.validate()?;
    match (&evidence.staged, &evidence.unstaged) {
        (SurfaceExtractionState::Absent, SurfaceExtractionState::Absent) => {
            Ok(B2AcceptedResult::NoTextualSurface { path_binding })
        }
        (SurfaceExtractionState::Extractable(_), _)
        | (_, SurfaceExtractionState::Extractable(_)) => Ok(B2AcceptedResult::Textual {
            evidence: ValidatedMeasuredEvidence::new(evidence)?,
            path_binding,
        }),
        (SurfaceExtractionState::NonExtractable(staged), SurfaceExtractionState::Absent) => {
            Ok(B2AcceptedResult::NotTextEligible {
                summary: NonTextEligibilitySummary::Staged(staged.clone()),
                path_binding,
            })
        }
        (SurfaceExtractionState::Absent, SurfaceExtractionState::NonExtractable(unstaged)) => {
            Ok(B2AcceptedResult::NotTextEligible {
                summary: NonTextEligibilitySummary::Unstaged(unstaged.clone()),
                path_binding,
            })
        }
        (
            SurfaceExtractionState::NonExtractable(staged),
            SurfaceExtractionState::NonExtractable(unstaged),
        ) => Ok(B2AcceptedResult::NotTextEligible {
            summary: NonTextEligibilitySummary::Both {
                staged: staged.clone(),
                unstaged: unstaged.clone(),
            },
            path_binding,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path() -> ValidatedInternalPathIdentity {
        ValidatedInternalPathIdentity::from_authoritative_path("-literal/file.txt".into()).unwrap()
    }

    fn non_extractable(reasons: &[NonExtractableReason]) -> SurfaceExtractionState {
        SurfaceExtractionState::NonExtractable(
            NonExtractableReasons::new(reasons.to_vec()).unwrap(),
        )
    }

    fn attributes() -> Vec<AttributeEligibilityEvidence> {
        GitSemanticAttribute::REQUIRED
            .iter()
            .copied()
            .map(|attribute| {
                AttributeEligibilityEvidence::new(attribute, GitAttributeState::Unspecified)
                    .unwrap()
            })
            .collect()
    }

    fn attributes_with(
        attribute: GitSemanticAttribute,
        state: GitAttributeState,
    ) -> Vec<AttributeEligibilityEvidence> {
        let mut values = attributes();
        let index = GitSemanticAttribute::REQUIRED
            .iter()
            .position(|candidate| *candidate == attribute)
            .unwrap();
        values[index] = AttributeEligibilityEvidence::new(attribute, state).unwrap();
        values
    }

    fn line(
        kind: TextualDiffLineKind,
        old: Option<LineEnding>,
        new: Option<LineEnding>,
    ) -> TextualDiffLine {
        TextualDiffLine {
            kind,
            text: "text".into(),
            old_no_newline_marker: matches!(old, Some(LineEnding::None)),
            new_no_newline_marker: matches!(new, Some(LineEnding::None)),
            old_line_ending: old,
            new_line_ending: new,
        }
    }

    fn extractable() -> SurfaceExtractionState {
        SurfaceExtractionState::Extractable(TextualSurfaceEvidence {
            old_mode: Some(RepositoryMode::Regular),
            new_mode: Some(RepositoryMode::Regular),
            hunks: vec![TextualHunk {
                old_start: 1,
                old_count: 1,
                new_start: 1,
                new_count: 1,
                lines: vec![line(
                    TextualDiffLineKind::Context,
                    Some(LineEnding::Lf),
                    Some(LineEnding::Lf),
                )],
            }],
            total_additions: 0,
            total_deletions: 0,
        })
    }

    fn evidence(
        staged: SurfaceExtractionState,
        unstaged: SurfaceExtractionState,
    ) -> TextualExtractionEvidence {
        TextualExtractionEvidence {
            path: path(),
            staged,
            unstaged,
            attributes: attributes(),
        }
    }

    fn measured() -> ValidatedMeasuredEvidence {
        ValidatedMeasuredEvidence::new(evidence(extractable(), SurfaceExtractionState::Absent))
            .unwrap()
    }

    fn path_binding() -> ValidatedPathBudgetBinding {
        let (request, resolved) = request_and_resolved();
        let evidence = evidence(
            SurfaceExtractionState::Absent,
            SurfaceExtractionState::Absent,
        );
        bind_validated_path_budget(&request, &resolved, &evidence).unwrap()
    }

    fn request_and_resolved() -> (
        BoundedTextualExtractionRequest,
        ResolvedTextualExtractionIdentity,
    ) {
        let project_id = ProjectId::new();
        let worktree_id = WorktreeId::new();
        let selection = NonAuthoritativePathSelection::new("-literal/file.txt".into()).unwrap();
        let request = BoundedTextualExtractionRequest::new(
            project_id.clone(),
            worktree_id.clone(),
            selection.clone(),
        );
        let resolved =
            ResolvedTextualExtractionIdentity::new(project_id, worktree_id, selection, path());
        (request, resolved)
    }

    #[test]
    fn request_identity_is_typed_single_path_and_literal_dash_is_valid() {
        let selection = NonAuthoritativePathSelection::new("-literal/file.txt".into()).unwrap();
        let request =
            BoundedTextualExtractionRequest::new(ProjectId::new(), WorktreeId::new(), selection);
        assert_eq!(request.selection.as_str(), "-literal/file.txt");
        assert_ne!(
            request.project_id.to_string(),
            request.worktree_id.to_string()
        );
        assert!(NonAuthoritativePathSelection::new("../escape".into()).is_err());
        assert!(
            NonAuthoritativePathSelection::new("x".repeat(MAX_SELECTED_PATH_BYTES + 1)).is_err()
        );
    }

    #[test]
    fn surface_states_and_reasons_are_not_interchangeable() {
        assert_ne!(
            SurfaceExtractionState::Absent,
            non_extractable(&[NonExtractableReason::Binary])
        );
        assert_ne!(NonExtractableReason::Binary, NonExtractableReason::ModeOnly);
        assert_ne!(
            NonExtractableReason::FilterConfigured,
            NonExtractableReason::DiffAttributeConfigured
        );
        assert!(evidence(
            extractable(),
            non_extractable(&[NonExtractableReason::Binary])
        )
        .validate()
        .is_ok());
        let (request, resolved) = request_and_resolved();
        assert!(matches!(
            accept_evidence(
                &request,
                &resolved,
                evidence(
                    non_extractable(&[NonExtractableReason::Binary]),
                    non_extractable(&[NonExtractableReason::ModeOnly])
                )
            )
            .unwrap(),
            B2AcceptedResult::NotTextEligible { .. }
        ));
    }

    #[test]
    fn equality_preserves_line_kinds_text_coordinates_and_newlines() {
        let base = evidence(extractable(), SurfaceExtractionState::Absent);
        let mut crlf = base.clone();
        if let SurfaceExtractionState::Extractable(surface) = &mut crlf.staged {
            surface.hunks[0].lines[0].old_line_ending = Some(LineEnding::CrLf);
            surface.hunks[0].lines[0].new_line_ending = Some(LineEnding::CrLf);
        }
        assert_ne!(base, crlf);
        let mut no_newline = base.clone();
        if let SurfaceExtractionState::Extractable(surface) = &mut no_newline.staged {
            surface.hunks[0].lines[0].old_line_ending = Some(LineEnding::None);
            surface.hunks[0].lines[0].old_no_newline_marker = true;
            surface.hunks[0].lines[0].new_line_ending = Some(LineEnding::None);
            surface.hunks[0].lines[0].new_no_newline_marker = true;
        }
        assert_ne!(base, no_newline);
        let mut old_only_no_newline = base.clone();
        if let SurfaceExtractionState::Extractable(surface) = &mut old_only_no_newline.staged {
            surface.hunks[0].lines[0].old_line_ending = Some(LineEnding::None);
            surface.hunks[0].lines[0].old_no_newline_marker = true;
        }
        assert_ne!(no_newline, old_only_no_newline);
        let mut coordinate = base.clone();
        if let SurfaceExtractionState::Extractable(surface) = &mut coordinate.staged {
            surface.hunks[0].old_start = 2;
        }
        assert_ne!(base, coordinate);
    }

    #[test]
    fn line_and_hunk_invariants_fail_closed() {
        let invalid_marker = TextualDiffLine {
            old_no_newline_marker: true,
            ..line(TextualDiffLineKind::Addition, None, Some(LineEnding::Lf))
        };
        assert!(validate_line(&invalid_marker).is_err());
        let invalid_control = TextualDiffLine {
            text: "bad\u{1b}".into(),
            ..line(
                TextualDiffLineKind::Context,
                Some(LineEnding::Lf),
                Some(LineEnding::Lf),
            )
        };
        assert!(validate_line(&invalid_control).is_err());
        let mut bad = extractable();
        if let SurfaceExtractionState::Extractable(surface) = &mut bad {
            surface.total_additions = 1;
        }
        assert!(validate_surface(&bad).is_err());

        let mut early_old_marker = extractable();
        if let SurfaceExtractionState::Extractable(surface) = &mut early_old_marker {
            surface.hunks[0].lines = vec![
                line(TextualDiffLineKind::Deletion, Some(LineEnding::None), None),
                line(TextualDiffLineKind::Deletion, Some(LineEnding::Lf), None),
            ];
            surface.hunks[0].old_count = 2;
            surface.hunks[0].new_count = 0;
            surface.total_deletions = 2;
        }
        assert!(validate_surface(&early_old_marker).is_err());

        let mut earlier_hunk_marker = extractable();
        if let SurfaceExtractionState::Extractable(surface) = &mut earlier_hunk_marker {
            surface.hunks = vec![
                TextualHunk {
                    old_start: 1,
                    old_count: 1,
                    new_start: 1,
                    new_count: 0,
                    lines: vec![line(
                        TextualDiffLineKind::Deletion,
                        Some(LineEnding::None),
                        None,
                    )],
                },
                TextualHunk {
                    old_start: 2,
                    old_count: 1,
                    new_start: 1,
                    new_count: 1,
                    lines: vec![line(
                        TextualDiffLineKind::Context,
                        Some(LineEnding::Lf),
                        Some(LineEnding::Lf),
                    )],
                },
            ];
            surface.total_deletions = 1;
        }
        assert!(validate_surface(&earlier_hunk_marker).is_err());

        let mut final_new_marker = extractable();
        if let SurfaceExtractionState::Extractable(surface) = &mut final_new_marker {
            surface.hunks[0].lines = vec![
                line(TextualDiffLineKind::Addition, None, Some(LineEnding::Lf)),
                line(TextualDiffLineKind::Addition, None, Some(LineEnding::None)),
            ];
            surface.hunks[0].old_count = 0;
            surface.hunks[0].new_count = 2;
            surface.total_additions = 2;
        }
        assert!(validate_surface(&final_new_marker).is_ok());
    }

    #[test]
    fn new_side_no_newline_finality_is_directly_regressed() {
        let mut same_hunk_addition = extractable();
        if let SurfaceExtractionState::Extractable(surface) = &mut same_hunk_addition {
            surface.hunks[0].lines = vec![
                line(TextualDiffLineKind::Addition, None, Some(LineEnding::None)),
                line(TextualDiffLineKind::Addition, None, Some(LineEnding::Lf)),
            ];
            surface.hunks[0].old_count = 0;
            surface.hunks[0].new_count = 2;
            surface.total_additions = 2;
        }
        assert_eq!(
            validate_surface(&same_hunk_addition),
            Err(B2Error::InvalidEvidence)
        );

        let mut same_hunk_context = extractable();
        if let SurfaceExtractionState::Extractable(surface) = &mut same_hunk_context {
            surface.hunks[0].lines = vec![
                line(TextualDiffLineKind::Addition, None, Some(LineEnding::None)),
                line(
                    TextualDiffLineKind::Context,
                    Some(LineEnding::Lf),
                    Some(LineEnding::Lf),
                ),
            ];
            surface.hunks[0].old_count = 1;
            surface.hunks[0].new_count = 2;
            surface.total_additions = 1;
        }
        assert_eq!(
            validate_surface(&same_hunk_context),
            Err(B2Error::InvalidEvidence)
        );

        let mut later_hunk = extractable();
        if let SurfaceExtractionState::Extractable(surface) = &mut later_hunk {
            surface.hunks = vec![
                TextualHunk {
                    old_start: 1,
                    old_count: 0,
                    new_start: 1,
                    new_count: 1,
                    lines: vec![line(
                        TextualDiffLineKind::Addition,
                        None,
                        Some(LineEnding::None),
                    )],
                },
                TextualHunk {
                    old_start: 1,
                    old_count: 0,
                    new_start: 2,
                    new_count: 1,
                    lines: vec![line(
                        TextualDiffLineKind::Addition,
                        None,
                        Some(LineEnding::Lf),
                    )],
                },
            ];
            surface.total_additions = 2;
        }
        assert_eq!(validate_surface(&later_hunk), Err(B2Error::InvalidEvidence));

        let mut later_hunk_context = extractable();
        if let SurfaceExtractionState::Extractable(surface) = &mut later_hunk_context {
            surface.hunks = vec![
                TextualHunk {
                    old_start: 1,
                    old_count: 0,
                    new_start: 1,
                    new_count: 1,
                    lines: vec![line(
                        TextualDiffLineKind::Addition,
                        None,
                        Some(LineEnding::None),
                    )],
                },
                TextualHunk {
                    old_start: 1,
                    old_count: 1,
                    new_start: 2,
                    new_count: 1,
                    lines: vec![line(
                        TextualDiffLineKind::Context,
                        Some(LineEnding::Lf),
                        Some(LineEnding::Lf),
                    )],
                },
            ];
            surface.total_additions = 1;
        }
        assert_eq!(
            validate_surface(&later_hunk_context),
            Err(B2Error::InvalidEvidence)
        );

        let mut cross_hunk_duplicate_marker = extractable();
        if let SurfaceExtractionState::Extractable(surface) = &mut cross_hunk_duplicate_marker {
            surface.hunks = vec![
                TextualHunk {
                    old_start: 1,
                    old_count: 0,
                    new_start: 1,
                    new_count: 1,
                    lines: vec![line(
                        TextualDiffLineKind::Addition,
                        None,
                        Some(LineEnding::None),
                    )],
                },
                TextualHunk {
                    old_start: 1,
                    old_count: 0,
                    new_start: 2,
                    new_count: 1,
                    lines: vec![line(
                        TextualDiffLineKind::Addition,
                        None,
                        Some(LineEnding::None),
                    )],
                },
            ];
            surface.total_additions = 2;
        }
        assert_eq!(
            validate_surface(&cross_hunk_duplicate_marker),
            Err(B2Error::InvalidEvidence)
        );

        let mut duplicate_marker = extractable();
        if let SurfaceExtractionState::Extractable(surface) = &mut duplicate_marker {
            surface.hunks[0].lines = vec![
                line(TextualDiffLineKind::Addition, None, Some(LineEnding::None)),
                line(TextualDiffLineKind::Addition, None, Some(LineEnding::None)),
            ];
            surface.hunks[0].old_count = 0;
            surface.hunks[0].new_count = 2;
            surface.total_additions = 2;
        }
        assert_eq!(
            validate_surface(&duplicate_marker),
            Err(B2Error::InvalidEvidence)
        );

        let marker_without_none = TextualDiffLine {
            new_no_newline_marker: true,
            ..line(TextualDiffLineKind::Addition, None, Some(LineEnding::Lf))
        };
        assert_eq!(
            validate_line(&marker_without_none),
            Err(B2Error::InvalidEvidence)
        );
        let none_without_marker = TextualDiffLine {
            new_line_ending: Some(LineEnding::None),
            new_no_newline_marker: false,
            ..line(TextualDiffLineKind::Addition, None, Some(LineEnding::Lf))
        };
        assert_eq!(
            validate_line(&none_without_marker),
            Err(B2Error::InvalidEvidence)
        );

        let mut old_only_after_new_final = extractable();
        if let SurfaceExtractionState::Extractable(surface) = &mut old_only_after_new_final {
            surface.hunks[0].lines = vec![
                line(TextualDiffLineKind::Addition, None, Some(LineEnding::None)),
                line(TextualDiffLineKind::Deletion, Some(LineEnding::Lf), None),
            ];
            surface.hunks[0].old_count = 1;
            surface.hunks[0].new_count = 1;
            surface.total_additions = 1;
            surface.total_deletions = 1;
        }
        assert!(validate_surface(&old_only_after_new_final).is_ok());

        let invalid_marker = TextualDiffLine {
            new_no_newline_marker: true,
            ..line(TextualDiffLineKind::Deletion, Some(LineEnding::Lf), None)
        };
        assert_eq!(
            validate_line(&invalid_marker),
            Err(B2Error::InvalidEvidence)
        );
    }

    #[test]
    fn all_required_attribute_states_are_exact_and_bounded() {
        for attribute in GitSemanticAttribute::REQUIRED {
            assert!(matches!(
                AttributeEligibilityEvidence::new(attribute, GitAttributeState::Unspecified)
                    .unwrap()
                    .decision,
                AttributeEligibilityDecision::Eligible
            ));
            for state in [
                GitAttributeState::Set,
                GitAttributeState::Unset,
                GitAttributeState::Value("configured".into()),
            ] {
                assert!(matches!(
                    AttributeEligibilityEvidence::new(attribute, state)
                        .unwrap()
                        .decision,
                    AttributeEligibilityDecision::NonExtractable(_)
                ));
            }
            assert_eq!(
                AttributeEligibilityEvidence::new(attribute, GitAttributeState::Deferred)
                    .map(|evidence| evidence.decision),
                if attribute == GitSemanticAttribute::Filter {
                    Ok(AttributeEligibilityDecision::NonExtractable(
                        AttributeIneligibilityReason::Deferred,
                    ))
                } else {
                    Err(B2Error::InvalidEvidence)
                }
            );
        }
        assert!(AttributeEligibilityEvidence::new(
            GitSemanticAttribute::Filter,
            GitAttributeState::Value("x".repeat(MAX_SEMANTIC_ATTRIBUTE_VALUE_BYTES + 1))
        )
        .is_err());
    }

    #[test]
    fn attribute_equality_and_completeness_detect_drift() {
        let base = attributes();
        let mut changed = base.clone();
        changed[0] =
            AttributeEligibilityEvidence::new(GitSemanticAttribute::Filter, GitAttributeState::Set)
                .unwrap();
        assert_ne!(base, changed);
        assert!(validate_attributes(&changed).is_ok());
        let mut duplicate = attributes();
        duplicate[1] = duplicate[0].clone();
        assert!(validate_attributes(&duplicate).is_err());

        let mut ineligible_attributes = attributes();
        ineligible_attributes[0] =
            AttributeEligibilityEvidence::new(GitSemanticAttribute::Filter, GitAttributeState::Set)
                .unwrap();
        let ineligible_extractable = TextualExtractionEvidence {
            attributes: ineligible_attributes,
            ..evidence(extractable(), SurfaceExtractionState::Absent)
        };
        assert_eq!(
            ineligible_extractable.validate(),
            Err(B2Error::InvalidEvidence)
        );

        for (index, state) in [
            GitAttributeState::Set,
            GitAttributeState::Value("configured".into()),
            GitAttributeState::Unset,
            GitAttributeState::Set,
            GitAttributeState::Value("lf".into()),
            GitAttributeState::Value("utf-16".into()),
        ]
        .into_iter()
        .enumerate()
        {
            let mut rejected = attributes();
            let attribute = GitSemanticAttribute::REQUIRED[index];
            rejected[index] = AttributeEligibilityEvidence::new(attribute, state).unwrap();
            assert_eq!(
                TextualExtractionEvidence {
                    attributes: rejected,
                    ..evidence(extractable(), SurfaceExtractionState::Absent)
                }
                .validate(),
                Err(B2Error::InvalidEvidence)
            );
        }
    }

    #[test]
    fn non_extractable_reasons_are_complete_and_attribute_coherent() {
        assert!(NonExtractableReasons::new(Vec::new()).is_err());
        assert!(NonExtractableReasons::new(vec![
            NonExtractableReason::Binary,
            NonExtractableReason::Binary,
        ])
        .is_err());

        let configured_mappings = [
            (
                GitSemanticAttribute::Filter,
                NonExtractableReason::FilterConfigured,
            ),
            (
                GitSemanticAttribute::Diff,
                NonExtractableReason::DiffAttributeConfigured,
            ),
            (
                GitSemanticAttribute::Text,
                NonExtractableReason::TextAttributeConfigured,
            ),
            (
                GitSemanticAttribute::Binary,
                NonExtractableReason::BinaryAttributeConfigured,
            ),
            (
                GitSemanticAttribute::Eol,
                NonExtractableReason::EolAttributeConfigured,
            ),
            (
                GitSemanticAttribute::WorkingTreeEncoding,
                NonExtractableReason::WorkingTreeEncodingConfigured,
            ),
        ];
        for (attribute, reason) in configured_mappings {
            for state in [
                GitAttributeState::Set,
                GitAttributeState::Unset,
                GitAttributeState::Value("configured".into()),
            ] {
                let coherent = TextualExtractionEvidence {
                    attributes: attributes_with(attribute, state),
                    ..evidence(non_extractable(&[reason]), SurfaceExtractionState::Absent)
                };
                assert!(coherent.validate().is_ok());
                let missing_reason = TextualExtractionEvidence {
                    attributes: coherent.attributes.clone(),
                    ..evidence(
                        non_extractable(&[NonExtractableReason::Binary]),
                        SurfaceExtractionState::Absent,
                    )
                };
                assert_eq!(missing_reason.validate(), Err(B2Error::InvalidEvidence));
            }
        }

        for false_reason in [
            NonExtractableReason::FilterConfigured,
            NonExtractableReason::FilterDeferred,
            NonExtractableReason::DiffAttributeConfigured,
            NonExtractableReason::TextAttributeConfigured,
            NonExtractableReason::BinaryAttributeConfigured,
            NonExtractableReason::EolAttributeConfigured,
            NonExtractableReason::WorkingTreeEncodingConfigured,
        ] {
            assert_eq!(
                evidence(
                    non_extractable(&[false_reason]),
                    SurfaceExtractionState::Absent,
                )
                .validate(),
                Err(B2Error::InvalidEvidence)
            );
        }

        let deferred = TextualExtractionEvidence {
            attributes: attributes_with(GitSemanticAttribute::Filter, GitAttributeState::Deferred),
            ..evidence(
                non_extractable(&[
                    NonExtractableReason::Binary,
                    NonExtractableReason::FilterDeferred,
                ]),
                SurfaceExtractionState::Absent,
            )
        };
        let (request, resolved) = request_and_resolved();
        assert!(matches!(
            accept_evidence(&request, &resolved, deferred).unwrap(),
            B2AcceptedResult::NotTextEligible {
                summary: NonTextEligibilitySummary::Staged(reasons),
                ..
            }
                if reasons.contains(NonExtractableReason::Binary)
                    && reasons.contains(NonExtractableReason::FilterDeferred)
        ));

        let invalid_deferred_reason = TextualExtractionEvidence {
            attributes: attributes_with(GitSemanticAttribute::Filter, GitAttributeState::Deferred),
            ..evidence(
                non_extractable(&[NonExtractableReason::FilterConfigured]),
                SurfaceExtractionState::Absent,
            )
        };
        assert_eq!(
            invalid_deferred_reason.validate(),
            Err(B2Error::InvalidEvidence)
        );

        let deferred_extractable = TextualExtractionEvidence {
            attributes: attributes_with(GitSemanticAttribute::Filter, GitAttributeState::Deferred),
            ..evidence(extractable(), SurfaceExtractionState::Absent)
        };
        assert_eq!(
            deferred_extractable.validate(),
            Err(B2Error::InvalidEvidence)
        );
        let deferred_both_surfaces = TextualExtractionEvidence {
            attributes: attributes_with(GitSemanticAttribute::Filter, GitAttributeState::Deferred),
            ..evidence(
                non_extractable(&[NonExtractableReason::FilterDeferred]),
                non_extractable(&[
                    NonExtractableReason::Binary,
                    NonExtractableReason::FilterDeferred,
                ]),
            )
        };
        let (request, resolved) = request_and_resolved();
        assert!(matches!(
            accept_evidence(&request, &resolved, deferred_both_surfaces).unwrap(),
            B2AcceptedResult::NotTextEligible {
                summary: NonTextEligibilitySummary::Both { staged, unstaged },
                ..
            }
                if staged.contains(NonExtractableReason::FilterDeferred)
                    && unstaged.contains(NonExtractableReason::FilterDeferred)
                    && unstaged.contains(NonExtractableReason::Binary)
        ));

        let mut combined_attributes = attributes();
        combined_attributes[1] = AttributeEligibilityEvidence::new(
            GitSemanticAttribute::Diff,
            GitAttributeState::Value("driver".into()),
        )
        .unwrap();
        combined_attributes[4] = AttributeEligibilityEvidence::new(
            GitSemanticAttribute::Eol,
            GitAttributeState::Value("lf".into()),
        )
        .unwrap();
        combined_attributes[5] = AttributeEligibilityEvidence::new(
            GitSemanticAttribute::WorkingTreeEncoding,
            GitAttributeState::Value("utf-16".into()),
        )
        .unwrap();
        let combined = TextualExtractionEvidence {
            attributes: combined_attributes,
            ..evidence(
                non_extractable(&[
                    NonExtractableReason::DiffAttributeConfigured,
                    NonExtractableReason::EolAttributeConfigured,
                    NonExtractableReason::WorkingTreeEncodingConfigured,
                ]),
                SurfaceExtractionState::Absent,
            )
        };
        assert!(combined.validate().is_ok());
        let missing_combined_reason = TextualExtractionEvidence {
            attributes: combined.attributes.clone(),
            ..evidence(
                non_extractable(&[
                    NonExtractableReason::DiffAttributeConfigured,
                    NonExtractableReason::EolAttributeConfigured,
                ]),
                SurfaceExtractionState::Absent,
            )
        };
        assert_eq!(
            missing_combined_reason.validate(),
            Err(B2Error::InvalidEvidence)
        );

        let mut deferred_and_configured =
            attributes_with(GitSemanticAttribute::Filter, GitAttributeState::Deferred);
        deferred_and_configured[1] = AttributeEligibilityEvidence::new(
            GitSemanticAttribute::Diff,
            GitAttributeState::Value("driver".into()),
        )
        .unwrap();
        let deferred_and_configured = TextualExtractionEvidence {
            attributes: deferred_and_configured,
            ..evidence(
                non_extractable(&[
                    NonExtractableReason::DiffAttributeConfigured,
                    NonExtractableReason::FilterDeferred,
                ]),
                SurfaceExtractionState::Absent,
            )
        };
        assert!(deferred_and_configured.validate().is_ok());
    }

    #[test]
    fn measured_evidence_owns_a_conservative_retention_charge() {
        let base = measured();
        assert!(base.retained_charge.0 > 0);
        assert_eq!(base.normalized_text_bytes, 4);
        assert_eq!(base.line_count, 1);
        assert_eq!(base.hunk_count, 1);
        assert_eq!(base.path_bytes, path().0.len());

        let mut more_text = evidence(extractable(), SurfaceExtractionState::Absent);
        if let SurfaceExtractionState::Extractable(surface) = &mut more_text.staged {
            surface.hunks[0].lines[0]
                .text
                .push_str(" more retained text");
        }
        let more_text = ValidatedMeasuredEvidence::new(more_text).unwrap();
        assert!(more_text.retained_charge.0 >= base.retained_charge.0);

        let mut more_hunks = evidence(extractable(), SurfaceExtractionState::Absent);
        if let SurfaceExtractionState::Extractable(surface) = &mut more_hunks.staged {
            surface.hunks.push(TextualHunk {
                old_start: 2,
                old_count: 1,
                new_start: 2,
                new_count: 1,
                lines: vec![line(
                    TextualDiffLineKind::Context,
                    Some(LineEnding::Lf),
                    Some(LineEnding::Lf),
                )],
            });
        }
        let more_hunks = ValidatedMeasuredEvidence::new(more_hunks).unwrap();
        assert!(more_hunks.retained_charge.0 >= base.retained_charge.0);

        let one_reason = ValidatedMeasuredEvidence::new(evidence(
            non_extractable(&[NonExtractableReason::Binary]),
            SurfaceExtractionState::Absent,
        ))
        .unwrap();
        let two_reasons = ValidatedMeasuredEvidence::new(evidence(
            non_extractable(&[NonExtractableReason::Binary, NonExtractableReason::ModeOnly]),
            SurfaceExtractionState::Absent,
        ))
        .unwrap();
        assert!(two_reasons.retained_charge.0 >= one_reason.retained_charge.0);

        let mut filter_value_attributes = attributes();
        filter_value_attributes[0] = AttributeEligibilityEvidence::new(
            GitSemanticAttribute::Filter,
            GitAttributeState::Value("configured-filter".into()),
        )
        .unwrap();
        let with_attribute_value = ValidatedMeasuredEvidence::new(TextualExtractionEvidence {
            attributes: filter_value_attributes,
            ..evidence(
                non_extractable(&[NonExtractableReason::FilterConfigured]),
                SurfaceExtractionState::Absent,
            )
        })
        .unwrap();
        let mut filter_set_attributes = attributes();
        filter_set_attributes[0] =
            AttributeEligibilityEvidence::new(GitSemanticAttribute::Filter, GitAttributeState::Set)
                .unwrap();
        let without_attribute_value = ValidatedMeasuredEvidence::new(TextualExtractionEvidence {
            attributes: filter_set_attributes,
            ..evidence(
                non_extractable(&[NonExtractableReason::FilterConfigured]),
                SurfaceExtractionState::Absent,
            )
        })
        .unwrap();
        assert!(
            with_attribute_value.retained_charge.0 >= without_attribute_value.retained_charge.0
        );

        let oversized = ValidatedMeasuredEvidence::new(TextualExtractionEvidence {
            path: path(),
            staged: SurfaceExtractionState::Extractable(TextualSurfaceEvidence {
                old_mode: Some(RepositoryMode::Regular),
                new_mode: Some(RepositoryMode::Regular),
                hunks: vec![TextualHunk {
                    old_start: 1,
                    old_count: 193,
                    new_start: 1,
                    new_count: 193,
                    lines: (0..193)
                        .map(|_| TextualDiffLine {
                            text: "x".repeat(MAX_DIFF_LINE_BYTES),
                            ..line(
                                TextualDiffLineKind::Context,
                                Some(LineEnding::Lf),
                                Some(LineEnding::Lf),
                            )
                        })
                        .collect(),
                }],
                total_additions: 0,
                total_deletions: 0,
            }),
            unstaged: SurfaceExtractionState::Absent,
            attributes: attributes(),
        })
        .unwrap();
        assert!(oversized.retained_charge.0 > MAX_TOTAL_RETAINED_EXTRACTION_BYTES);

        let mut budget = B2RequestBudget::default();
        let binding = path_binding();
        let token = budget
            .reserve_measured_evidence(&binding, &base, 0)
            .unwrap();
        assert_eq!(budget.usage.retained_extraction, base.retained_charge.0);
        budget.release_retained(token).unwrap();
        let usage = budget.usage.clone();
        assert!(matches!(
            budget.reserve_measured_evidence(&binding, &oversized, 0),
            Err(B2BudgetError::Limit(B2BudgetLimit::NormalizedText))
        ));
        assert_eq!(budget.usage, usage);
    }

    #[test]
    fn reservation_uses_only_evidence_derived_text_line_and_hunk_metrics() {
        let mut dual_surface = evidence(extractable(), extractable());
        if let SurfaceExtractionState::Extractable(surface) = &mut dual_surface.staged {
            surface.hunks[0].lines[0].text = "staged".into();
        }
        if let SurfaceExtractionState::Extractable(surface) = &mut dual_surface.unstaged {
            surface.hunks.push(TextualHunk {
                old_start: 2,
                old_count: 1,
                new_start: 2,
                new_count: 1,
                lines: vec![line(
                    TextualDiffLineKind::Context,
                    Some(LineEnding::Lf),
                    Some(LineEnding::Lf),
                )],
            });
            surface.hunks[1].lines[0].text = "unstaged".into();
        }
        let measured = ValidatedMeasuredEvidence::new(dual_surface).unwrap();
        assert_eq!(
            measured.normalized_text_bytes,
            "staged".len() + "text".len() + "unstaged".len()
        );
        assert_eq!(measured.line_count, 3);
        assert_eq!(measured.hunk_count, 3);

        let binding = path_binding();
        let mut budget = B2RequestBudget::default();
        let token = budget
            .reserve_measured_evidence(&binding, &measured, 17)
            .unwrap();
        assert_eq!(budget.usage.parsed_diff, 17);
        assert_eq!(budget.usage.normalized_text, measured.normalized_text_bytes);
        assert_eq!(budget.usage.lines, measured.line_count);
        assert_eq!(budget.usage.hunks, measured.hunk_count);
        budget.release_retained(token).unwrap();

        let mut line_limit = B2RequestBudget::default();
        let line_binding = path_binding();
        line_limit.usage.lines = MAX_TOTAL_LINES_PER_REQUEST - measured.line_count;
        let token = line_limit
            .reserve_measured_evidence(&line_binding, &measured, 0)
            .unwrap();
        line_limit.release_retained(token).unwrap();
        let before = line_limit.usage.clone();
        assert!(matches!(
            line_limit.reserve_measured_evidence(&line_binding, &measured, 0),
            Err(B2BudgetError::Limit(B2BudgetLimit::Lines))
        ));
        assert_eq!(line_limit.usage, before);

        let mut hunk_limit = B2RequestBudget::default();
        let hunk_binding = path_binding();
        hunk_limit.usage.hunks = MAX_TOTAL_HUNKS_PER_REQUEST - measured.hunk_count;
        let token = hunk_limit
            .reserve_measured_evidence(&hunk_binding, &measured, 0)
            .unwrap();
        hunk_limit.release_retained(token).unwrap();
        let before = hunk_limit.usage.clone();
        assert!(matches!(
            hunk_limit.reserve_measured_evidence(&hunk_binding, &measured, 0),
            Err(B2BudgetError::Limit(B2BudgetLimit::Hunks))
        ));
        assert_eq!(hunk_limit.usage, before);
    }

    #[test]
    fn budget_caps_are_checked_and_transactional() {
        let mut budget = B2RequestBudget::default();
        assert!(budget
            .charge_child(MAX_CHILD_STDOUT_BYTES, MAX_CHILD_STDERR_BYTES)
            .is_ok());
        let usage = budget.usage.clone();
        assert_eq!(
            budget.charge_child(MAX_CHILD_STDOUT_BYTES + 1, 0),
            Err(B2BudgetError::Limit(B2BudgetLimit::CapturedStdout))
        );
        assert_eq!(budget.usage, usage);
        for _ in 1..MAX_TOTAL_GIT_CHILDREN_PER_REQUEST {
            budget.charge_child(0, 0).unwrap();
        }
        assert_eq!(
            budget.charge_child(0, 0),
            Err(B2BudgetError::Limit(B2BudgetLimit::Children))
        );
    }

    #[test]
    fn budget_shares_e_passes_and_releases_only_retained_memory() {
        let mut budget = B2RequestBudget::default();
        let measured = measured();
        let binding = path_binding();
        let e1 = budget
            .reserve_measured_evidence(&binding, &measured, MIB)
            .unwrap();
        let e2 = budget
            .reserve_measured_evidence(&binding, &measured, MIB)
            .unwrap();
        assert!(matches!(
            budget.reserve_measured_evidence(&binding, &measured, 1),
            Err(B2BudgetError::Limit(B2BudgetLimit::RetainedStructures))
        ));
        let processed = budget.usage.parsed_diff;
        budget.release_retained(e1).unwrap();
        assert_eq!(budget.usage.parsed_diff, processed);
        let e3 = budget
            .reserve_measured_evidence(&binding, &measured, MIB)
            .unwrap();
        assert!(matches!(
            budget.reserve_measured_evidence(&binding, &measured, 1),
            Err(B2BudgetError::Limit(B2BudgetLimit::ParsedDiff))
        ));
        let mut other_budget = B2RequestBudget::default();
        let other_usage = other_budget.usage.clone();
        assert_eq!(
            other_budget.release_retained(e2),
            Err(B2BudgetError::ReleaseUnderflow)
        );
        assert_eq!(other_budget.usage, other_usage);
        let unknown = RetentionToken {
            ledger: Arc::clone(&budget.ledger),
            allocation_id: u64::MAX,
        };
        let usage_before_unknown = budget.usage.clone();
        assert_eq!(
            budget.release_retained(unknown),
            Err(B2BudgetError::ReleaseUnderflow)
        );
        assert_eq!(budget.usage, usage_before_unknown);
        assert_eq!(budget.usage.retained_structures, 2);
        budget.release_retained(e3).unwrap();
    }

    #[test]
    fn aggregate_budgets_reject_exactly_the_next_capture_byte() {
        let mut capture = B2RequestBudget::default();
        for _ in 0..4 {
            capture.charge_child(MAX_CHILD_STDOUT_BYTES, 0).unwrap();
        }
        assert_eq!(
            capture.charge_child(1, 0),
            Err(B2BudgetError::Limit(B2BudgetLimit::CapturedStdout))
        );
    }

    #[test]
    fn authoritative_path_is_bound_once_and_all_outcomes_share_the_budget() {
        let binding = path_binding();
        assert_eq!(binding.path_bytes, path().0.len());
        assert!(binding.path_bytes > 0);
        let measured = measured();
        let mut budget = B2RequestBudget::default();

        let e1 = budget
            .reserve_measured_evidence(&binding, &measured, 0)
            .unwrap();
        assert_eq!(budget.usage.paths, binding.path_bytes);
        let e2 = budget
            .reserve_measured_evidence(&binding, &measured, 0)
            .unwrap();
        assert_eq!(budget.usage.paths, binding.path_bytes);
        budget.release_retained(e1).unwrap();
        let e3 = budget
            .reserve_measured_evidence(&binding, &measured, 0)
            .unwrap();
        assert_eq!(budget.usage.paths, binding.path_bytes);
        budget.release_retained(e2).unwrap();
        budget.release_retained(e3).unwrap();
        assert_eq!(budget.usage.paths, binding.path_bytes);

        let (request, resolved) = request_and_resolved();
        let no_text = accept_evidence(
            &request,
            &resolved,
            evidence(
                SurfaceExtractionState::Absent,
                SurfaceExtractionState::Absent,
            ),
        )
        .unwrap();
        let mut non_text_budget = B2RequestBudget::default();
        non_text_budget.bind_accepted_outcome(&no_text).unwrap();
        assert_eq!(non_text_budget.usage.paths, binding.path_bytes);
        non_text_budget.bind_accepted_outcome(&no_text).unwrap();
        assert_eq!(non_text_budget.usage.paths, binding.path_bytes);

        let not_eligible = accept_evidence(
            &request,
            &resolved,
            evidence(
                non_extractable(&[NonExtractableReason::Binary]),
                SurfaceExtractionState::Absent,
            ),
        )
        .unwrap();
        non_text_budget
            .bind_accepted_outcome(&not_eligible)
            .unwrap();
        assert_eq!(non_text_budget.usage.paths, binding.path_bytes);

        let exact_path = "p".repeat(MAX_SELECTED_PATH_BYTES);
        let project_id = ProjectId::new();
        let worktree_id = WorktreeId::new();
        let selection = NonAuthoritativePathSelection::new(exact_path.clone()).unwrap();
        let exact_request = BoundedTextualExtractionRequest::new(
            project_id.clone(),
            worktree_id.clone(),
            selection.clone(),
        );
        let exact_resolved = ResolvedTextualExtractionIdentity::new(
            project_id,
            worktree_id,
            selection,
            ValidatedInternalPathIdentity::from_authoritative_path(exact_path).unwrap(),
        );
        let exact_outcome = accept_evidence(
            &exact_request,
            &exact_resolved,
            TextualExtractionEvidence {
                path: exact_resolved.authoritative_path_identity.clone(),
                staged: SurfaceExtractionState::Absent,
                unstaged: SurfaceExtractionState::Absent,
                attributes: attributes(),
            },
        )
        .unwrap();
        let mut exact_budget = B2RequestBudget::default();
        exact_budget.bind_accepted_outcome(&exact_outcome).unwrap();
        assert_eq!(exact_budget.usage.paths, MAX_SELECTED_PATH_BYTES);
        assert!(
            NonAuthoritativePathSelection::new("p".repeat(MAX_SELECTED_PATH_BYTES + 1)).is_err()
        );
        assert!(ValidatedInternalPathIdentity::from_authoritative_path(
            "p".repeat(MAX_SELECTED_PATH_BYTES + 1)
        )
        .is_err());

        let mut wrong_project = binding.clone();
        wrong_project.project_id = ProjectId::new();
        let before = budget.usage.clone();
        let bound_before = budget.bound_path.clone();
        assert!(matches!(
            budget.reserve_measured_evidence(&wrong_project, &measured, 0),
            Err(B2BudgetError::PathBindingMismatch)
        ));
        assert_eq!(budget.usage, before);
        assert_eq!(budget.bound_path, bound_before);

        let mut wrong_worktree = binding.clone();
        wrong_worktree.worktree_id = WorktreeId::new();
        assert!(matches!(
            budget.reserve_measured_evidence(&wrong_worktree, &measured, 0),
            Err(B2BudgetError::PathBindingMismatch)
        ));
        assert_eq!(budget.usage, before);
        assert_eq!(budget.bound_path, bound_before);

        let mut wrong_path = binding.clone();
        wrong_path.authoritative_path_identity =
            ValidatedInternalPathIdentity::from_authoritative_path("other.txt".into()).unwrap();
        wrong_path.path_bytes = wrong_path.authoritative_path_identity.0.len();
        assert!(matches!(
            budget.reserve_measured_evidence(&wrong_path, &measured, 0),
            Err(B2BudgetError::PathBindingMismatch)
        ));
        assert_eq!(budget.usage, before);
        assert_eq!(budget.bound_path, bound_before);

        let mut transactional = B2RequestBudget::default();
        transactional.usage.lines = MAX_TOTAL_LINES_PER_REQUEST;
        let before = transactional.usage.clone();
        assert!(matches!(
            transactional.reserve_measured_evidence(&binding, &measured, 0),
            Err(B2BudgetError::Limit(B2BudgetLimit::Lines))
        ));
        assert_eq!(transactional.usage, before);
        assert!(transactional.bound_path.is_none());
    }

    #[test]
    fn checked_arithmetic_overflow_and_invalid_evidence_never_mutate_acceptance() {
        assert_eq!(
            checked_add_usize(usize::MAX, 1, usize::MAX, B2BudgetLimit::ParsedDiff),
            Err(B2BudgetError::ArithmeticOverflow)
        );
        let too_long = TextualDiffLine {
            text: "x".repeat(MAX_DIFF_LINE_BYTES + 1),
            ..line(
                TextualDiffLineKind::Context,
                Some(LineEnding::Lf),
                Some(LineEnding::Lf),
            )
        };
        assert!(validate_line(&too_long).is_err());
    }

    #[test]
    fn accepted_results_never_turn_errors_into_empty_success() {
        let (request, resolved) = request_and_resolved();
        let accepted = accept_evidence(
            &request,
            &resolved,
            evidence(
                SurfaceExtractionState::Absent,
                SurfaceExtractionState::Absent,
            ),
        )
        .unwrap();
        assert!(matches!(
            accepted,
            B2AcceptedResult::NoTextualSurface { .. }
        ));
        let invalid = TextualExtractionEvidence {
            attributes: Vec::new(),
            ..evidence(
                SurfaceExtractionState::Absent,
                SurfaceExtractionState::Absent,
            )
        };
        assert_eq!(
            accept_evidence(&request, &resolved, invalid),
            Err(B2Error::InvalidEvidence)
        );
        assert!(matches!(
            accept_evidence(
                &request,
                &resolved,
                evidence(
                    non_extractable(&[NonExtractableReason::Binary]),
                    SurfaceExtractionState::Absent,
                ),
            )
            .unwrap(),
            B2AcceptedResult::NotTextEligible { .. }
        ));
        assert!(matches!(
            accept_evidence(
                &request,
                &resolved,
                evidence(
                    extractable(),
                    non_extractable(&[NonExtractableReason::Binary]),
                ),
            )
            .unwrap(),
            B2AcceptedResult::Textual { .. }
        ));
        let wrong_request = BoundedTextualExtractionRequest::new(
            ProjectId::new(),
            request.worktree_id.clone(),
            request.selection.clone(),
        );
        assert_eq!(
            accept_evidence(
                &wrong_request,
                &resolved,
                evidence(extractable(), SurfaceExtractionState::Absent)
            ),
            Err(B2Error::InvalidRequest)
        );

        let wrong_worktree = BoundedTextualExtractionRequest::new(
            request.project_id.clone(),
            WorktreeId::new(),
            request.selection.clone(),
        );
        assert_eq!(
            accept_evidence(
                &wrong_worktree,
                &resolved,
                evidence(extractable(), SurfaceExtractionState::Absent)
            ),
            Err(B2Error::InvalidRequest)
        );
        let different_selection = BoundedTextualExtractionRequest::new(
            request.project_id.clone(),
            request.worktree_id.clone(),
            NonAuthoritativePathSelection::new("other.txt".into()).unwrap(),
        );
        assert_eq!(
            accept_evidence(
                &different_selection,
                &resolved,
                evidence(extractable(), SurfaceExtractionState::Absent)
            ),
            Err(B2Error::InvalidRequest)
        );
        let wrong_path = TextualExtractionEvidence {
            path: ValidatedInternalPathIdentity::from_authoritative_path("other.txt".into())
                .unwrap(),
            ..evidence(extractable(), SurfaceExtractionState::Absent)
        };
        assert_eq!(
            accept_evidence(&request, &resolved, wrong_path),
            Err(B2Error::InvalidRequest)
        );
        assert_ne!(B2Error::ChangeStale, B2Error::OutputOverflow);
        assert_ne!(B2Error::Timeout, B2Error::OperationCancelled);
    }
}
#[test]
fn b2_b0_fixture_conformance_smoke_and_unicode_alias_rejection() {
    let positive =
        include_bytes!("../../../../docs/fixtures/phase-3c-b2-b0/positive/P01-simple.patch");
    let negative = include_bytes!(
        "../../../../docs/fixtures/phase-3c-b2-b0/negative/N13-count-mismatch.patch"
    );
    let parsed =
        parse_private_diff_pair("file.txt".into(), &[], positive).expect("positive fixture");
    assert!(parsed.textual_available);
    assert_eq!(
        (parsed.unstaged_additions, parsed.unstaged_deletions),
        (1, 1)
    );
    assert!(matches!(
        parse_private_diff_pair("file.txt".into(), &[], negative),
        Err(B2Error::InvalidEvidence)
    ));
    assert!(NonAuthoritativePathSelection::new("composed-é.txt".into()).is_err());
    assert!(NonAuthoritativePathSelection::new("decomposed-e\u{301}.txt".into()).is_err());
}
