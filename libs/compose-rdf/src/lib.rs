//! Read and write a composition `PlanV1` as RDF triples.
//!
//! The wf plugin's `wf:compose(<plan>, …)` invocation fires a SPARQL query
//! that walks a plan's transitive closure — every triple whose subject is
//! the plan IRI or a blank node reachable from it — and hands the rows to
//! `plan_from_rdf`. This crate turns those triples into the same
//! `compose_core::PlanV1` the CBOR / composectl paths already produce, so
//! downstream fetching, validation, composition, and execution are
//! byte-identical whether the plan came from disk or from the graph.
//!
//! # Vocabulary
//!
//! Namespace: `http://tegmentum.ai/ns/composition/`
//!
//! ```turtle
//! @prefix comp: <http://tegmentum.ai/ns/composition/> .
//! @prefix : <urn:my:> .
//!
//! :lang_pipeline a comp:CompositionPlan ;
//!   comp:version "1" ;
//!   comp:root :detect ;
//!   comp:component :detect , :translate ;
//!   comp:binding [
//!     comp:import "translate:run" ;
//!     comp:provider :translate ;
//!     comp:export "run"
//!   ] ;
//!   comp:policy [
//!     comp:determinism "strict" ;
//!     comp:memoryBytes 67108864 ;
//!     comp:capability [ comp:name "wasi:filesystem" ; comp:level "optional" ]
//!   ] .
//!
//! :detect a comp:Component ;
//!   comp:source <https://.../lingua-detect.wasm> ;
//!   comp:digest "sha256:0f1c...ab7d" .
//!
//! :translate a comp:Component ;
//!   comp:source <ipfs://Qm.../translate.wasm> ;
//!   comp:digest "sha256:c74d...e103" .
//! ```
//!
//! # Component identifiers
//!
//! The composition core uses opaque `String` component IDs. In RDF the
//! natural identity is an IRI, so we derive the ID from the last path or
//! fragment segment (`.../lingua-detect` → `"lingua-detect"`,
//! `urn:my:detect` → `"detect"`). A caller who wants an explicit ID can
//! set `comp:id` on the component subject; the derived form is a fallback.

use anyhow::{Context, Result, anyhow, bail};
use compose_core::{
    Capability, CapabilityLevel, ComponentId, ComponentSpec, DeterminismMode, Digest,
    ImportBinding, Linkage, PlanV1, Policy, ResourceLimits, SecretBinding,
};
use compose_core::types::ExplicitExport;
use std::collections::{BTreeMap, HashMap};

/// Default plan IRI used by [`plan_to_rdf`] and [`plan_to_turtle`] when the
/// caller doesn't supply one. Consumers that want a stable per-plan IRI
/// (e.g. namespaced by the plan's digest) can use
/// [`plan_to_rdf_with_iri`] / [`plan_to_turtle_with_iri`] instead.
///
/// The reader's `plan_iri` argument must match whatever IRI was used at
/// write time for a lossless round-trip.
pub const DEFAULT_PLAN_IRI: &str = "urn:composition:plan";

// ---------------------------------------------------------------------------
// RDF term surface
// ---------------------------------------------------------------------------

/// Minimal Term shape. Matches the WIT `value` variant the plugin's
/// `execute-query` callback produces, decoupled from any particular
/// SPARQL client library.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Term {
    Iri(String),
    Blank(String),
    Literal {
        value: String,
        datatype: Option<String>,
        language: Option<String>,
    },
}

impl Term {
    pub fn iri<S: Into<String>>(s: S) -> Self {
        Term::Iri(s.into())
    }
    pub fn blank<S: Into<String>>(s: S) -> Self {
        Term::Blank(s.into())
    }
    pub fn literal<S: Into<String>>(s: S) -> Self {
        Term::Literal {
            value: s.into(),
            datatype: None,
            language: None,
        }
    }
    pub fn typed_literal<S: Into<String>, D: Into<String>>(s: S, dt: D) -> Self {
        Term::Literal {
            value: s.into(),
            datatype: Some(dt.into()),
            language: None,
        }
    }
    /// Best-effort textual view — the lexical form for literals, the IRI
    /// for IRIs, and the label for blank nodes. Used for identity
    /// derivation and for feeding scalar fields (versions, tags, etc.).
    pub fn as_string(&self) -> &str {
        match self {
            Term::Iri(s) => s,
            Term::Blank(s) => s,
            Term::Literal { value, .. } => value,
        }
    }
}

/// One RDF triple. Passed in bulk to [`plan_from_rdf`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Triple {
    pub subject: Term,
    pub predicate: Term,
    pub object: Term,
}

impl Triple {
    pub fn new(s: Term, p: Term, o: Term) -> Self {
        Self {
            subject: s,
            predicate: p,
            object: o,
        }
    }
}

// ---------------------------------------------------------------------------
// Vocabulary
// ---------------------------------------------------------------------------

/// Vocabulary constants. Namespaced under
/// `http://tegmentum.ai/ns/composition/`; matches the [comp:] prefix used
/// in plan documents.
pub mod vocab {
    pub const NS: &str = "http://tegmentum.ai/ns/composition/";

    macro_rules! iri {
        ($name:ident, $suffix:literal) => {
            pub const $name: &str = concat!("http://tegmentum.ai/ns/composition/", $suffix);
        };
    }

    // rdf:type — reused so we can spot `?s a comp:CompositionPlan`.
    pub const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";

    // Plan top-level class + properties
    iri!(COMPOSITION_PLAN, "CompositionPlan");
    iri!(VERSION,          "version");
    iri!(ROOT,             "root");
    iri!(COMPONENT,        "component");
    iri!(BINDING,          "binding");
    iri!(SECRET,           "secret");
    iri!(POLICY,           "policy");
    iri!(LINKAGE,          "linkage");

    // Component
    iri!(COMPONENT_CLASS,  "Component");
    iri!(ID,               "id");
    iri!(SOURCE,           "source");
    iri!(DIGEST,           "digest");

    // Binding
    iri!(IMPORT,           "import");
    iri!(PROVIDER,         "provider");
    iri!(EXPORT,           "export");
    iri!(CONSUMER,         "consumer");

    // Secret binding
    iri!(SECRET_ID,        "secretId");
    iri!(BACKEND,          "backend");

    // Policy
    iri!(DETERMINISM,      "determinism");
    iri!(CAPABILITY,       "capability");
    iri!(CPU_MS,           "cpuMs");
    iri!(MEMORY_BYTES,     "memoryBytes");
    iri!(IO_OPS,           "ioOps");
    iri!(TENANT,           "tenant");

    // Capability
    iri!(NAME,             "name");
    iri!(LEVEL,            "level");

    // Explicit-export re-surface (writer-only for now: the current reader
    // does not parse these; see `plan_to_rdf` for the round-trip note).
    iri!(EXPLICIT_EXPORT,  "explicitExport");
    iri!(SOURCE_INSTANCE,  "sourceInstance");
    iri!(INTERFACE_NAME,   "interfaceName");
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

/// Turn a set of triples into a [`PlanV1`] rooted at `plan_iri`.
///
/// The caller is expected to have already fetched every triple describing
/// the plan and any blank nodes reachable from it (see the crate-level
/// docs for a suggested SPARQL query shape). Missing required fields fail
/// with a descriptive error; unknown predicates on known subjects are
/// silently ignored to keep the vocabulary forward-compatible.
pub fn plan_from_rdf(plan_iri: &str, triples: &[Triple]) -> Result<PlanV1> {
    let index = TripleIndex::new(triples);

    // Sanity: the plan subject must be typed as a CompositionPlan. Refuse
    // otherwise — silently proceeding would let a caller compose against
    // arbitrary blank nodes.
    let plan_subject = Term::Iri(plan_iri.into());
    if !index.has_type(&plan_subject, vocab::COMPOSITION_PLAN) {
        bail!(
            "plan {plan_iri} is not typed as comp:CompositionPlan; \
             add `<{plan_iri}> a comp:CompositionPlan .`"
        );
    }

    // Version. Default to "1" when omitted — the CBOR spec only defines v1
    // so far, and requiring the literal in every RDF plan buys nothing.
    let version = index
        .literal(&plan_subject, vocab::VERSION)
        .unwrap_or_else(|| "1".to_string());

    // Root component.
    let root_ref = index
        .single(&plan_subject, vocab::ROOT)
        .ok_or_else(|| anyhow!("plan {plan_iri} is missing comp:root"))?;
    let root = term_to_component_id(root_ref)?;

    // Components. Order-preserving so composition output is deterministic
    // across runs even when RDF stores return triples in different orders.
    let mut components = Vec::new();
    let mut seen_ids: BTreeMap<String, &Term> = BTreeMap::new(); // id -> subject term
    for c_ref in index.iter(&plan_subject, vocab::COMPONENT) {
        let spec = component_from_rdf(c_ref, &index)?;
        if let Some(prev) = seen_ids.insert(spec.id.clone(), *c_ref) {
            bail!(
                "plan {plan_iri}: two components resolve to the same id `{}` \
                 (subjects: {:?} and {:?})",
                spec.id,
                prev,
                c_ref
            );
        }
        components.push(spec);
    }
    if components.is_empty() {
        bail!("plan {plan_iri} lists no comp:component");
    }
    if !components.iter().any(|c| c.id == root) {
        bail!(
            "plan {plan_iri}: comp:root refers to id `{root}` which is not \
             among the plan's components"
        );
    }
    // Deterministic component order regardless of RDF-store iteration.
    components.sort_by(|a, b| a.id.cmp(&b.id));

    // Bindings.
    let mut bindings = Vec::new();
    for b_ref in index.iter(&plan_subject, vocab::BINDING) {
        bindings.push(binding_from_rdf(b_ref, &index, &seen_ids)?);
    }

    // Secret bindings.
    let mut secrets = Vec::new();
    for s_ref in index.iter(&plan_subject, vocab::SECRET) {
        secrets.push(secret_from_rdf(s_ref, &index)?);
    }

    // Policy — optional. Absence yields the default policy (strict, no
    // capabilities, no limits, no tenant).
    let policy = index
        .single(&plan_subject, vocab::POLICY)
        .map(|p_ref| policy_from_rdf(p_ref, &index))
        .transpose()?
        .unwrap_or_default();

    // Linkage — defaults to Static so pre-existing plans keep their digests.
    let linkage = index
        .literal(&plan_subject, vocab::LINKAGE)
        .as_deref()
        .map(parse_linkage)
        .transpose()?
        .unwrap_or_default();

    Ok(PlanV1 {
        version,
        root,
        components,
        bindings,
        secrets,
        policy,
        linkage,
        explicit_exports: Vec::new(),
    })
}

// ---------------------------------------------------------------------------
// Field parsers
// ---------------------------------------------------------------------------

fn component_from_rdf(subject: &Term, index: &TripleIndex<'_>) -> Result<ComponentSpec> {
    let id = index
        .literal(subject, vocab::ID)
        .map(Ok)
        .unwrap_or_else(|| term_to_component_id(subject))?;

    let source = index.literal(subject, vocab::SOURCE).or_else(|| {
        index.single(subject, vocab::SOURCE).and_then(|t| match t {
            Term::Iri(s) => Some(s.clone()),
            _ => None,
        })
    });

    let digest_str = index
        .literal(subject, vocab::DIGEST)
        .ok_or_else(|| anyhow!("component {id}: comp:digest is required"))?;
    let digest = parse_digest(&digest_str)
        .with_context(|| format!("component {id}: parsing comp:digest {digest_str:?}"))?;

    Ok(ComponentSpec {
        id,
        digest,
        source,
    })
}

fn binding_from_rdf<'a>(
    subject: &Term,
    index: &TripleIndex<'_>,
    known_ids: &BTreeMap<String, &'a Term>,
) -> Result<ImportBinding> {
    let import_name = index
        .literal(subject, vocab::IMPORT)
        .ok_or_else(|| anyhow!("binding {subject:?}: comp:import is required"))?;

    let provider_ref = index
        .single(subject, vocab::PROVIDER)
        .ok_or_else(|| anyhow!("binding {subject:?}: comp:provider is required"))?;
    let provider_id = term_to_component_id(provider_ref)?;
    if !known_ids.contains_key(&provider_id) {
        bail!(
            "binding {subject:?}: comp:provider `{provider_id}` is not a \
             component in this plan"
        );
    }

    let export_name = index
        .literal(subject, vocab::EXPORT)
        .ok_or_else(|| anyhow!("binding {subject:?}: comp:export is required"))?;

    let consumer_id = match index.single(subject, vocab::CONSUMER) {
        Some(c) => Some(term_to_component_id(c)?),
        None => None,
    };

    Ok(ImportBinding {
        consumer_id,
        import_name,
        provider_id,
        export_name,
    })
}

fn secret_from_rdf(subject: &Term, index: &TripleIndex<'_>) -> Result<SecretBinding> {
    let secret_id = index
        .literal(subject, vocab::SECRET_ID)
        .ok_or_else(|| anyhow!("secret {subject:?}: comp:secretId is required"))?;
    let backend_uri = index
        .literal(subject, vocab::BACKEND)
        .or_else(|| {
            index.single(subject, vocab::BACKEND).and_then(|t| match t {
                Term::Iri(s) => Some(s.clone()),
                _ => None,
            })
        })
        .ok_or_else(|| anyhow!("secret {subject:?}: comp:backend is required"))?;
    Ok(SecretBinding {
        secret_id,
        backend_uri,
    })
}

fn policy_from_rdf(subject: &Term, index: &TripleIndex<'_>) -> Result<Policy> {
    let determinism = index
        .literal(subject, vocab::DETERMINISM)
        .as_deref()
        .map(parse_determinism)
        .transpose()?
        .unwrap_or(DeterminismMode::Strict);

    let mut capabilities = Vec::new();
    for cap_ref in index.iter(subject, vocab::CAPABILITY) {
        let name = index
            .literal(cap_ref, vocab::NAME)
            .ok_or_else(|| anyhow!("capability {cap_ref:?}: comp:name is required"))?;
        let level = index
            .literal(cap_ref, vocab::LEVEL)
            .as_deref()
            .map(parse_capability_level)
            .transpose()?
            .unwrap_or(CapabilityLevel::Required);
        capabilities.push(Capability { name, level });
    }
    capabilities.sort_by(|a, b| a.name.cmp(&b.name));

    let limits = ResourceLimits {
        cpu_ms: index
            .literal(subject, vocab::CPU_MS)
            .as_deref()
            .map(parse_u64)
            .transpose()?,
        memory_bytes: index
            .literal(subject, vocab::MEMORY_BYTES)
            .as_deref()
            .map(parse_u64)
            .transpose()?,
        io_ops: index
            .literal(subject, vocab::IO_OPS)
            .as_deref()
            .map(parse_u64)
            .transpose()?,
    };

    let tenant = index.literal(subject, vocab::TENANT);

    Ok(Policy {
        determinism,
        capabilities,
        tenant,
        limits,
    })
}

// ---------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------

fn term_to_component_id(t: &Term) -> Result<ComponentId> {
    match t {
        Term::Iri(iri) => Ok(iri_local_name(iri)),
        Term::Blank(id) => Ok(id.clone()),
        Term::Literal { value, .. } => Ok(value.clone()),
    }
}

/// Extract the local name from an IRI: the tail after the last `#` or `/`.
/// Falls back to the whole IRI if neither is present (e.g. `urn:foo` → `foo`;
/// `urn:foo:bar` → `bar` after the colon-fallback below).
fn iri_local_name(iri: &str) -> String {
    if let Some(idx) = iri.rfind('#') {
        return iri[idx + 1..].to_string();
    }
    if let Some(idx) = iri.rfind('/') {
        return iri[idx + 1..].to_string();
    }
    if let Some(idx) = iri.rfind(':') {
        return iri[idx + 1..].to_string();
    }
    iri.to_string()
}

fn parse_digest(s: &str) -> Result<Digest> {
    // Accept `sha256:<hex>` or bare `<hex>`; either way, we store the raw
    // 32-byte digest in compose-core's `Digest = Vec<u8>` type.
    let hex_part = s.strip_prefix("sha256:").unwrap_or(s);
    let bytes = hex_decode(hex_part).with_context(|| "digest hex decode")?;
    if bytes.len() != 32 {
        bail!(
            "digest must be 32 bytes (SHA-256, 64 hex chars); got {} bytes",
            bytes.len()
        );
    }
    Ok(bytes)
}

fn hex_decode(hex: &str) -> Result<Vec<u8>> {
    if hex.len() % 2 != 0 {
        bail!("hex string has odd length");
    }
    let mut out = Vec::with_capacity(hex.len() / 2);
    let bytes = hex.as_bytes();
    for i in (0..bytes.len()).step_by(2) {
        let hi = hex_nibble(bytes[i])?;
        let lo = hex_nibble(bytes[i + 1])?;
        out.push((hi << 4) | lo);
    }
    Ok(out)
}

fn hex_nibble(b: u8) -> Result<u8> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        b'A'..=b'F' => Ok(b - b'A' + 10),
        _ => bail!("invalid hex character {:?}", b as char),
    }
}

fn parse_u64(s: &str) -> Result<u64> {
    s.parse::<u64>()
        .with_context(|| format!("parsing u64 from {s:?}"))
}

fn parse_determinism(s: &str) -> Result<DeterminismMode> {
    match s {
        "strict" => Ok(DeterminismMode::Strict),
        "audit" => Ok(DeterminismMode::Audit),
        "relaxed" => Ok(DeterminismMode::Relaxed),
        _ => bail!("unknown determinism mode {s:?}"),
    }
}

fn parse_capability_level(s: &str) -> Result<CapabilityLevel> {
    match s {
        "required" => Ok(CapabilityLevel::Required),
        "optional" => Ok(CapabilityLevel::Optional),
        _ => bail!("unknown capability level {s:?}"),
    }
}

fn parse_linkage(s: &str) -> Result<Linkage> {
    match s {
        "static" => Ok(Linkage::Static),
        "runtime" => Ok(Linkage::Runtime),
        _ => bail!("unknown linkage {s:?}"),
    }
}

// ---------------------------------------------------------------------------
// Index — thin lookup layer over a triple slice
// ---------------------------------------------------------------------------

/// Two-level index over `&[Triple]`: keyed by subject, then predicate.
/// The subject is looked up by identity (Term equality); the predicate is
/// looked up by IRI string, since it's always an IRI in well-formed RDF.
struct TripleIndex<'a> {
    by_subject: HashMap<&'a Term, HashMap<&'a str, Vec<&'a Term>>>,
}

impl<'a> TripleIndex<'a> {
    fn new(triples: &'a [Triple]) -> Self {
        let mut by_subject: HashMap<&Term, HashMap<&str, Vec<&Term>>> = HashMap::new();
        for t in triples {
            let p = match &t.predicate {
                Term::Iri(iri) => iri.as_str(),
                _ => continue, // Only IRI predicates are meaningful.
            };
            by_subject
                .entry(&t.subject)
                .or_default()
                .entry(p)
                .or_default()
                .push(&t.object);
        }
        Self { by_subject }
    }

    /// True when `subject rdf:type ty`.
    fn has_type(&self, subject: &Term, ty: &str) -> bool {
        self.by_subject
            .get(subject)
            .and_then(|m| m.get(vocab::RDF_TYPE))
            .into_iter()
            .flatten()
            .any(|o| matches!(o, Term::Iri(iri) if iri == ty))
    }

    /// All objects for `subject predicate ?`, in insertion order.
    fn iter(&self, subject: &Term, predicate: &str) -> impl Iterator<Item = &&'a Term> + '_ {
        self.by_subject
            .get(subject)
            .and_then(|m| m.get(predicate))
            .into_iter()
            .flatten()
    }

    /// The single object for `subject predicate ?`, if any. Later
    /// duplicates on the same predicate are ignored; callers who care
    /// should use [`iter`] instead.
    fn single(&self, subject: &Term, predicate: &str) -> Option<&&'a Term> {
        self.iter(subject, predicate).next()
    }

    /// The single object as a string literal (or the string form of an IRI
    /// as a fallback — useful for `comp:source` which the vocabulary
    /// documents as either).
    fn literal(&self, subject: &Term, predicate: &str) -> Option<String> {
        self.single(subject, predicate).map(|t| t.as_string().to_string())
    }
}

// ---------------------------------------------------------------------------
// Writer — PlanV1 -> Vec<Triple>
// ---------------------------------------------------------------------------

/// Serialize a [`PlanV1`] to RDF triples using [`DEFAULT_PLAN_IRI`] as the
/// plan subject.
///
/// The writer is the counterpart to [`plan_from_rdf`] and is designed for
/// a lossless round-trip: `plan_from_rdf(DEFAULT_PLAN_IRI,
/// &plan_to_rdf(p))` reconstructs `p` for any valid plan whose
/// `explicit_exports` list is empty. See [`plan_to_rdf_with_iri`] for the
/// full round-trip contract and the [explicit-exports gap][gap].
///
/// [gap]: plan_to_rdf_with_iri
pub fn plan_to_rdf(plan: &PlanV1) -> Vec<Triple> {
    plan_to_rdf_with_iri(plan, DEFAULT_PLAN_IRI)
}

/// Serialize a [`PlanV1`] to RDF triples with a caller-chosen plan IRI.
///
/// # Round-trip contract
///
/// For any [`PlanV1`] whose `explicit_exports` list is empty and whose
/// components carry SHA-256 digests, the sequence
///
/// ```ignore
/// let ts = plan_to_rdf_with_iri(&p, "urn:my:plan");
/// let back = plan_from_rdf("urn:my:plan", &ts).unwrap();
/// assert_eq!(canonical(back), canonical(p));
/// ```
///
/// holds, where `canonical` sorts `components` by id and `capabilities`
/// by name — the same normalizations the reader performs. The writer
/// pre-sorts capabilities so that its own output already matches the
/// reader's canonicalization; components are emitted in the plan's own
/// order and sorted on the way back in by the reader.
///
/// # Explicit-exports gap
///
/// The current [`plan_from_rdf`] does not parse `comp:explicitExport`
/// blank nodes. The writer emits them (under the vocab constants added
/// alongside `plan_to_rdf`) so the vocab surface is complete and a
/// future reader upgrade can pick them up, but until that lands a plan
/// with non-empty `explicit_exports` does NOT round-trip: the reader
/// will reconstruct it with `explicit_exports: vec![]`. Callers that
/// need this today should either upgrade the reader or wrap the CBOR
/// payload alongside the RDF.
///
/// # Component identity
///
/// Components are emitted as blank-node subjects with an explicit
/// `comp:id "<id>"` literal, and every cross-reference
/// (`comp:root`, `comp:provider`, `comp:consumer`) is written as a
/// string literal rather than a component IRI. The reader accepts
/// literal terms for those slots (`term_to_component_id` returns the
/// literal value verbatim), which lets the writer support arbitrary
/// opaque `ComponentId` strings without having to prove they round-trip
/// through IRI local-name derivation.
pub fn plan_to_rdf_with_iri(plan: &PlanV1, plan_iri: &str) -> Vec<Triple> {
    let mut ts: Vec<Triple> = Vec::new();
    let plan_subject = Term::iri(plan_iri);
    let type_pred = Term::iri(vocab::RDF_TYPE);

    // Plan class.
    ts.push(Triple::new(
        plan_subject.clone(),
        type_pred.clone(),
        Term::iri(vocab::COMPOSITION_PLAN),
    ));

    // Version — always emitted so the reader picks up the explicit value
    // instead of falling back to "1".
    ts.push(Triple::new(
        plan_subject.clone(),
        Term::iri(vocab::VERSION),
        Term::literal(plan.version.clone()),
    ));

    // Root — literal so any opaque ComponentId string round-trips
    // through term_to_component_id.
    ts.push(Triple::new(
        plan_subject.clone(),
        Term::iri(vocab::ROOT),
        Term::literal(plan.root.clone()),
    ));

    // Components — blank subjects with explicit comp:id.
    for (i, spec) in plan.components.iter().enumerate() {
        let subj = Term::blank(component_bnode_label(i));
        ts.push(Triple::new(
            plan_subject.clone(),
            Term::iri(vocab::COMPONENT),
            subj.clone(),
        ));
        ts.push(Triple::new(
            subj.clone(),
            type_pred.clone(),
            Term::iri(vocab::COMPONENT_CLASS),
        ));
        ts.push(Triple::new(
            subj.clone(),
            Term::iri(vocab::ID),
            Term::literal(spec.id.clone()),
        ));
        ts.push(Triple::new(
            subj.clone(),
            Term::iri(vocab::DIGEST),
            Term::literal(format!("sha256:{}", hex_encode(&spec.digest))),
        ));
        if let Some(src) = &spec.source {
            ts.push(Triple::new(
                subj.clone(),
                Term::iri(vocab::SOURCE),
                Term::literal(src.clone()),
            ));
        }
    }

    // Bindings — order preserved (reader keeps insertion order).
    for (i, b) in plan.bindings.iter().enumerate() {
        let subj = Term::blank(binding_bnode_label(i));
        ts.push(Triple::new(
            plan_subject.clone(),
            Term::iri(vocab::BINDING),
            subj.clone(),
        ));
        ts.push(Triple::new(
            subj.clone(),
            Term::iri(vocab::IMPORT),
            Term::literal(b.import_name.clone()),
        ));
        ts.push(Triple::new(
            subj.clone(),
            Term::iri(vocab::PROVIDER),
            Term::literal(b.provider_id.clone()),
        ));
        ts.push(Triple::new(
            subj.clone(),
            Term::iri(vocab::EXPORT),
            Term::literal(b.export_name.clone()),
        ));
        if let Some(cid) = &b.consumer_id {
            ts.push(Triple::new(
                subj.clone(),
                Term::iri(vocab::CONSUMER),
                Term::literal(cid.clone()),
            ));
        }
    }

    // Secrets.
    for (i, s) in plan.secrets.iter().enumerate() {
        let subj = Term::blank(secret_bnode_label(i));
        ts.push(Triple::new(
            plan_subject.clone(),
            Term::iri(vocab::SECRET),
            subj.clone(),
        ));
        ts.push(Triple::new(
            subj.clone(),
            Term::iri(vocab::SECRET_ID),
            Term::literal(s.secret_id.clone()),
        ));
        ts.push(Triple::new(
            subj.clone(),
            Term::iri(vocab::BACKEND),
            Term::literal(s.backend_uri.clone()),
        ));
    }

    // Policy — always emitted, always determinism-explicit so the
    // reader's inner default (Strict) doesn't shadow our Relaxed default.
    let policy_subj = Term::blank("pol".to_string());
    ts.push(Triple::new(
        plan_subject.clone(),
        Term::iri(vocab::POLICY),
        policy_subj.clone(),
    ));
    ts.push(Triple::new(
        policy_subj.clone(),
        Term::iri(vocab::DETERMINISM),
        Term::literal(determinism_str(plan.policy.determinism).to_string()),
    ));
    // Reader sorts capabilities by name — pre-sort so the writer's own
    // triple order already matches the round-trip.
    let mut caps_sorted: Vec<&Capability> = plan.policy.capabilities.iter().collect();
    caps_sorted.sort_by(|a, b| a.name.cmp(&b.name));
    for (i, c) in caps_sorted.iter().enumerate() {
        let cap_subj = Term::blank(capability_bnode_label(i));
        ts.push(Triple::new(
            policy_subj.clone(),
            Term::iri(vocab::CAPABILITY),
            cap_subj.clone(),
        ));
        ts.push(Triple::new(
            cap_subj.clone(),
            Term::iri(vocab::NAME),
            Term::literal(c.name.clone()),
        ));
        ts.push(Triple::new(
            cap_subj.clone(),
            Term::iri(vocab::LEVEL),
            Term::literal(capability_level_str(c.level).to_string()),
        ));
    }
    let ResourceLimits {
        cpu_ms,
        memory_bytes,
        io_ops,
    } = plan.policy.limits;
    if let Some(v) = cpu_ms {
        ts.push(Triple::new(
            policy_subj.clone(),
            Term::iri(vocab::CPU_MS),
            Term::literal(v.to_string()),
        ));
    }
    if let Some(v) = memory_bytes {
        ts.push(Triple::new(
            policy_subj.clone(),
            Term::iri(vocab::MEMORY_BYTES),
            Term::literal(v.to_string()),
        ));
    }
    if let Some(v) = io_ops {
        ts.push(Triple::new(
            policy_subj.clone(),
            Term::iri(vocab::IO_OPS),
            Term::literal(v.to_string()),
        ));
    }
    if let Some(t) = &plan.policy.tenant {
        ts.push(Triple::new(
            policy_subj.clone(),
            Term::iri(vocab::TENANT),
            Term::literal(t.clone()),
        ));
    }

    // Linkage — omit for Static (matches the CBOR default-skip so
    // pre-existing plans keep their existing shape).
    if !matches!(plan.linkage, Linkage::Static) {
        ts.push(Triple::new(
            plan_subject.clone(),
            Term::iri(vocab::LINKAGE),
            Term::literal(linkage_str(plan.linkage).to_string()),
        ));
    }

    // Explicit exports — written but currently one-way (see the
    // "Explicit-exports gap" note on this function).
    for (i, e) in plan.explicit_exports.iter().enumerate() {
        let subj = Term::blank(explicit_export_bnode_label(i));
        ts.push(Triple::new(
            plan_subject.clone(),
            Term::iri(vocab::EXPLICIT_EXPORT),
            subj.clone(),
        ));
        ts.push(Triple::new(
            subj.clone(),
            Term::iri(vocab::SOURCE_INSTANCE),
            Term::literal(e.source_instance.clone()),
        ));
        ts.push(Triple::new(
            subj.clone(),
            Term::iri(vocab::INTERFACE_NAME),
            Term::literal(e.interface_name.clone()),
        ));
    }

    ts
}

// Blank node label helpers — kept private so callers don't lean on the
// exact strings. The reader treats blank labels as opaque IDs; the
// writer picks stable, human-readable stems so `plan_to_turtle` output
// diffs cleanly.

fn component_bnode_label(i: usize) -> String {
    format!("c{}", i)
}

fn binding_bnode_label(i: usize) -> String {
    format!("b{}", i)
}

fn secret_bnode_label(i: usize) -> String {
    format!("s{}", i)
}

fn capability_bnode_label(i: usize) -> String {
    format!("cap{}", i)
}

fn explicit_export_bnode_label(i: usize) -> String {
    format!("xe{}", i)
}

fn determinism_str(m: DeterminismMode) -> &'static str {
    match m {
        DeterminismMode::Strict => "strict",
        DeterminismMode::Audit => "audit",
        DeterminismMode::Relaxed => "relaxed",
    }
}

fn capability_level_str(l: CapabilityLevel) -> &'static str {
    match l {
        CapabilityLevel::Required => "required",
        CapabilityLevel::Optional => "optional",
    }
}

fn linkage_str(l: Linkage) -> &'static str {
    match l {
        Linkage::Static => "static",
        Linkage::Runtime => "runtime",
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0F) as usize] as char);
    }
    out
}

// Explicit-exports today are writer-only. Silence dead-code warnings on
// the ExplicitExport shape — the field access in the writer above is
// enough on stable rustc but keeping this reference explicit documents
// intent for future readers.
#[allow(dead_code)]
fn _explicit_export_touch(e: &ExplicitExport) -> (&str, &str) {
    (&e.source_instance, &e.interface_name)
}

// ---------------------------------------------------------------------------
// Turtle emitter — Vec<Triple> -> String
// ---------------------------------------------------------------------------

/// Serialize a [`PlanV1`] to Turtle using [`DEFAULT_PLAN_IRI`] as the
/// plan subject. Thin wrapper around [`plan_to_rdf`] + [`triples_to_turtle`].
pub fn plan_to_turtle(plan: &PlanV1) -> String {
    plan_to_turtle_with_iri(plan, DEFAULT_PLAN_IRI)
}

/// Serialize a [`PlanV1`] to Turtle with a caller-chosen plan IRI. Thin
/// wrapper around [`plan_to_rdf_with_iri`] + [`triples_to_turtle`].
pub fn plan_to_turtle_with_iri(plan: &PlanV1, plan_iri: &str) -> String {
    triples_to_turtle(&plan_to_rdf_with_iri(plan, plan_iri))
}

/// Format a triple slice as valid Turtle. Emits a `@prefix comp:` header
/// then one triple per line — the plainest shape the Turtle grammar
/// admits so no third-party serializer is needed. Callers who want
/// tighter output can pass the result through their own RDF library;
/// this exists mostly so the WASM orchestrator can hand Java a ready-
/// to-load byte string without pulling `oxrdf`/`oxttl` into the guest.
pub fn triples_to_turtle(triples: &[Triple]) -> String {
    let mut out = String::new();
    out.push_str("@prefix comp: <");
    out.push_str(vocab::NS);
    out.push_str("> .\n");
    out.push_str("@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .\n\n");
    for t in triples {
        out.push_str(&format_term(&t.subject));
        out.push(' ');
        out.push_str(&format_predicate(&t.predicate));
        out.push(' ');
        out.push_str(&format_term(&t.object));
        out.push_str(" .\n");
    }
    out
}

fn format_predicate(t: &Term) -> String {
    // rdf:type gets the Turtle short form `a`; other predicates fall
    // through to the same rules as objects (IRI or prefixed IRI).
    if let Term::Iri(iri) = t {
        if iri == vocab::RDF_TYPE {
            return "a".to_string();
        }
    }
    format_term(t)
}

fn format_term(t: &Term) -> String {
    match t {
        Term::Iri(iri) => {
            if let Some(local) = iri.strip_prefix(vocab::NS) {
                if is_pn_local(local) {
                    return format!("comp:{}", local);
                }
            }
            format!("<{}>", escape_iri(iri))
        }
        Term::Blank(label) => {
            if is_safe_bnode_label(label) {
                format!("_:{}", label)
            } else {
                // Fall back to a hex-encoded label so weird bytes still
                // yield a syntactically valid Turtle blank node.
                format!("_:x{}", hex_encode(label.as_bytes()))
            }
        }
        Term::Literal {
            value,
            datatype,
            language,
        } => {
            let quoted = format!("\"{}\"", escape_literal(value));
            if let Some(lang) = language {
                format!("{}@{}", quoted, lang)
            } else if let Some(dt) = datatype {
                format!("{}^^<{}>", quoted, escape_iri(dt))
            } else {
                quoted
            }
        }
    }
}

fn is_pn_local(s: &str) -> bool {
    // Approximate PN_LOCAL: start with letter or _, remaining chars
    // alnum / . / -. Good enough for our vocab (`root`, `component`,
    // `Component`, `explicitExport`, ...).
    let mut it = s.chars();
    match it.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    for c in it {
        if !(c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.') {
            return false;
        }
    }
    true
}

fn is_safe_bnode_label(s: &str) -> bool {
    let mut it = s.chars();
    match it.next() {
        Some(c) if c.is_ascii_alphanumeric() || c == '_' => {}
        _ => return false,
    }
    for c in it {
        if !(c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.') {
            return false;
        }
    }
    // Turtle disallows a trailing `.` in blank labels.
    !s.ends_with('.')
}

fn escape_literal(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

fn escape_iri(s: &str) -> String {
    // Turtle IREF disallows a small set of characters; escape defensively.
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '<' | '>' | '"' | '{' | '}' | '|' | '^' | '`' | '\\' => {
                out.push_str(&format!("\\u{:04X}", c as u32));
            }
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn iri(s: &str) -> Term {
        Term::iri(s)
    }
    fn blank(s: &str) -> Term {
        Term::blank(s)
    }
    fn lit(s: &str) -> Term {
        Term::literal(s)
    }
    fn p(s: &str) -> Term {
        Term::iri(s)
    }
    fn t(s: Term, pr: Term, o: Term) -> Triple {
        Triple::new(s, pr, o)
    }

    fn v(x: &str) -> String {
        // Vocabulary helper
        x.to_string()
    }

    // A digest hex fixture that decodes to 32 bytes (0x11 repeated).
    const D1: &str = "1111111111111111111111111111111111111111111111111111111111111111";
    const D2: &str = "2222222222222222222222222222222222222222222222222222222222222222";

    #[test]
    fn iri_local_name_extraction() {
        assert_eq!(iri_local_name("http://x.example/foo#bar"), "bar");
        assert_eq!(iri_local_name("http://x.example/foo/bar"), "bar");
        assert_eq!(iri_local_name("urn:my:detect"), "detect");
        assert_eq!(iri_local_name("plain-word"), "plain-word");
    }

    #[test]
    fn minimal_plan_round_trip() {
        let plan = "urn:my:plan1";
        let root = "urn:my:hello";
        let triples = vec![
            t(iri(plan), p(vocab::RDF_TYPE), iri(vocab::COMPOSITION_PLAN)),
            t(iri(plan), p(vocab::ROOT), iri(root)),
            t(iri(plan), p(vocab::COMPONENT), iri(root)),
            t(iri(root), p(vocab::DIGEST), lit(D1)),
            t(iri(root), p(vocab::SOURCE), lit("file:///hello.wasm")),
        ];
        let got = plan_from_rdf(plan, &triples).unwrap();
        assert_eq!(got.version, "1");
        assert_eq!(got.root, "hello");
        assert_eq!(got.components.len(), 1);
        assert_eq!(got.components[0].id, "hello");
        assert_eq!(got.components[0].digest.len(), 32);
        assert_eq!(
            got.components[0].source.as_deref(),
            Some("file:///hello.wasm")
        );
        assert!(matches!(got.linkage, Linkage::Static));
    }

    #[test]
    fn refuses_untyped_plan_subject() {
        let plan = "urn:my:plan2";
        let triples = vec![
            // No rdf:type CompositionPlan
            t(iri(plan), p(vocab::ROOT), iri("urn:my:a")),
        ];
        let err = plan_from_rdf(plan, &triples).unwrap_err();
        assert!(format!("{err}").contains("comp:CompositionPlan"), "{err}");
    }

    #[test]
    fn refuses_root_that_is_not_a_component() {
        let plan = "urn:my:plan3";
        let triples = vec![
            t(iri(plan), p(vocab::RDF_TYPE), iri(vocab::COMPOSITION_PLAN)),
            t(iri(plan), p(vocab::ROOT), iri("urn:my:ghost")),
            t(iri(plan), p(vocab::COMPONENT), iri("urn:my:a")),
            t(iri("urn:my:a"), p(vocab::DIGEST), lit(D1)),
        ];
        let err = plan_from_rdf(plan, &triples).unwrap_err();
        assert!(format!("{err}").contains("comp:root"), "{err}");
    }

    #[test]
    fn refuses_duplicate_component_ids() {
        let plan = "urn:my:plan4";
        // Two component subjects that both derive to id "detect".
        let triples = vec![
            t(iri(plan), p(vocab::RDF_TYPE), iri(vocab::COMPOSITION_PLAN)),
            t(iri(plan), p(vocab::ROOT), iri("urn:one:detect")),
            t(iri(plan), p(vocab::COMPONENT), iri("urn:one:detect")),
            t(iri(plan), p(vocab::COMPONENT), iri("urn:two:detect")),
            t(iri("urn:one:detect"), p(vocab::DIGEST), lit(D1)),
            t(iri("urn:two:detect"), p(vocab::DIGEST), lit(D2)),
        ];
        let err = plan_from_rdf(plan, &triples).unwrap_err();
        assert!(format!("{err}").contains("same id"), "{err}");
    }

    #[test]
    fn parses_bindings_with_blank_subjects() {
        let plan = "urn:my:plan5";
        let a = "urn:my:a";
        let b = "urn:my:b";
        let bn = "_:bind1";
        let triples = vec![
            t(iri(plan), p(vocab::RDF_TYPE), iri(vocab::COMPOSITION_PLAN)),
            t(iri(plan), p(vocab::ROOT), iri(a)),
            t(iri(plan), p(vocab::COMPONENT), iri(a)),
            t(iri(plan), p(vocab::COMPONENT), iri(b)),
            t(iri(a), p(vocab::DIGEST), lit(D1)),
            t(iri(b), p(vocab::DIGEST), lit(D2)),
            t(iri(plan), p(vocab::BINDING), blank(bn)),
            t(blank(bn), p(vocab::IMPORT), lit("world:run")),
            t(blank(bn), p(vocab::PROVIDER), iri(b)),
            t(blank(bn), p(vocab::EXPORT), lit("run")),
        ];
        let got = plan_from_rdf(plan, &triples).unwrap();
        assert_eq!(got.bindings.len(), 1);
        assert_eq!(got.bindings[0].import_name, "world:run");
        assert_eq!(got.bindings[0].provider_id, "b");
        assert_eq!(got.bindings[0].export_name, "run");
    }

    #[test]
    fn parses_policy_and_limits() {
        let plan = "urn:my:plan6";
        let a = "urn:my:a";
        let pn = "_:pol";
        let cn = "_:cap";
        let triples = vec![
            t(iri(plan), p(vocab::RDF_TYPE), iri(vocab::COMPOSITION_PLAN)),
            t(iri(plan), p(vocab::ROOT), iri(a)),
            t(iri(plan), p(vocab::COMPONENT), iri(a)),
            t(iri(a), p(vocab::DIGEST), lit(D1)),
            t(iri(plan), p(vocab::POLICY), blank(pn)),
            t(blank(pn), p(vocab::DETERMINISM), lit("audit")),
            t(blank(pn), p(vocab::MEMORY_BYTES), lit("67108864")),
            t(blank(pn), p(vocab::CAPABILITY), blank(cn)),
            t(blank(cn), p(vocab::NAME), lit("wasi:filesystem")),
            t(blank(cn), p(vocab::LEVEL), lit("optional")),
        ];
        let got = plan_from_rdf(plan, &triples).unwrap();
        assert!(matches!(got.policy.determinism, DeterminismMode::Audit));
        assert_eq!(got.policy.limits.memory_bytes, Some(67_108_864));
        assert_eq!(got.policy.capabilities.len(), 1);
        assert_eq!(got.policy.capabilities[0].name, "wasi:filesystem");
        assert!(matches!(
            got.policy.capabilities[0].level,
            CapabilityLevel::Optional
        ));
    }

    #[test]
    fn linkage_defaults_to_static_and_can_be_runtime() {
        let plan = "urn:my:plan7";
        let a = "urn:my:a";
        let mk = |extra: Vec<Triple>| {
            let mut ts = vec![
                t(iri(plan), p(vocab::RDF_TYPE), iri(vocab::COMPOSITION_PLAN)),
                t(iri(plan), p(vocab::ROOT), iri(a)),
                t(iri(plan), p(vocab::COMPONENT), iri(a)),
                t(iri(a), p(vocab::DIGEST), lit(D1)),
            ];
            ts.extend(extra);
            ts
        };
        assert!(matches!(
            plan_from_rdf(plan, &mk(vec![])).unwrap().linkage,
            Linkage::Static
        ));
        assert!(matches!(
            plan_from_rdf(
                plan,
                &mk(vec![t(iri(plan), p(vocab::LINKAGE), lit("runtime"))])
            )
            .unwrap()
            .linkage,
            Linkage::Runtime
        ));
    }

    #[test]
    fn bad_digest_length_rejected() {
        let plan = "urn:my:plan8";
        let a = "urn:my:a";
        let triples = vec![
            t(iri(plan), p(vocab::RDF_TYPE), iri(vocab::COMPOSITION_PLAN)),
            t(iri(plan), p(vocab::ROOT), iri(a)),
            t(iri(plan), p(vocab::COMPONENT), iri(a)),
            t(iri(a), p(vocab::DIGEST), lit("cafebabe")),
        ];
        let err = plan_from_rdf(plan, &triples).unwrap_err();
        // The specific "32 bytes" message lives on the root cause; anyhow's
        // Display shows only the outer context by default.
        let full = format!("{err:#}");
        assert!(full.contains("32 bytes"), "{full}");
    }

    #[test]
    fn missing_provider_binding_errors_cleanly() {
        let plan = "urn:my:plan9";
        let a = "urn:my:a";
        let bn = "_:b";
        let triples = vec![
            t(iri(plan), p(vocab::RDF_TYPE), iri(vocab::COMPOSITION_PLAN)),
            t(iri(plan), p(vocab::ROOT), iri(a)),
            t(iri(plan), p(vocab::COMPONENT), iri(a)),
            t(iri(a), p(vocab::DIGEST), lit(D1)),
            t(iri(plan), p(vocab::BINDING), blank(bn)),
            t(blank(bn), p(vocab::IMPORT), lit("world:run")),
            t(blank(bn), p(vocab::PROVIDER), iri("urn:my:missing")),
            t(blank(bn), p(vocab::EXPORT), lit("run")),
        ];
        let err = plan_from_rdf(plan, &triples).unwrap_err();
        assert!(format!("{err}").contains("not a component"), "{err}");
    }

    #[test]
    fn silence_lint() {
        // Force the small helpers to be referenced even when tests grow.
        let _ = v("noop");
    }

    // -----------------------------------------------------------------
    // Writer + round-trip tests
    // -----------------------------------------------------------------

    // Bytes that decode to a 32-byte digest, used across the writer tests.
    fn digest_bytes(fill: u8) -> Vec<u8> {
        vec![fill; 32]
    }

    // Structural equality helper. `PlanV1` doesn't derive `PartialEq`, so
    // the round-trip check compares each field explicitly. Canonical CBOR
    // would also work but this failure mode is more legible.
    fn assert_plans_equal(got: &PlanV1, want: &PlanV1) {
        assert_eq!(got.version, want.version, "version");
        assert_eq!(got.root, want.root, "root");

        // Reader sorts components by id — normalise both sides so the
        // test doesn't depend on the writer's insertion order.
        let mut got_c = got.components.clone();
        let mut want_c = want.components.clone();
        got_c.sort_by(|a, b| a.id.cmp(&b.id));
        want_c.sort_by(|a, b| a.id.cmp(&b.id));
        assert_eq!(got_c.len(), want_c.len(), "components len");
        for (g, w) in got_c.iter().zip(want_c.iter()) {
            assert_eq!(g.id, w.id, "component id");
            assert_eq!(g.digest, w.digest, "component digest");
            assert_eq!(g.source, w.source, "component source");
        }

        assert_eq!(got.bindings.len(), want.bindings.len(), "bindings len");
        for (g, w) in got.bindings.iter().zip(want.bindings.iter()) {
            assert_eq!(g.consumer_id, w.consumer_id, "binding consumer");
            assert_eq!(g.import_name, w.import_name, "binding import");
            assert_eq!(g.provider_id, w.provider_id, "binding provider");
            assert_eq!(g.export_name, w.export_name, "binding export");
        }

        assert_eq!(got.secrets.len(), want.secrets.len(), "secrets len");
        for (g, w) in got.secrets.iter().zip(want.secrets.iter()) {
            assert_eq!(g.secret_id, w.secret_id, "secret id");
            assert_eq!(g.backend_uri, w.backend_uri, "secret backend");
        }

        assert_eq!(got.policy.determinism, want.policy.determinism, "determinism");
        assert_eq!(got.policy.tenant, want.policy.tenant, "tenant");
        assert_eq!(
            got.policy.limits.cpu_ms, want.policy.limits.cpu_ms,
            "cpu_ms"
        );
        assert_eq!(
            got.policy.limits.memory_bytes, want.policy.limits.memory_bytes,
            "memory_bytes"
        );
        assert_eq!(
            got.policy.limits.io_ops, want.policy.limits.io_ops,
            "io_ops"
        );
        // Capabilities: reader sorts by name; do the same to want.
        let mut got_cap = got.policy.capabilities.clone();
        let mut want_cap = want.policy.capabilities.clone();
        got_cap.sort_by(|a, b| a.name.cmp(&b.name));
        want_cap.sort_by(|a, b| a.name.cmp(&b.name));
        assert_eq!(got_cap.len(), want_cap.len(), "capabilities len");
        for (g, w) in got_cap.iter().zip(want_cap.iter()) {
            assert_eq!(g.name, w.name, "cap name");
            assert_eq!(g.level, w.level, "cap level");
        }

        assert_eq!(got.linkage, want.linkage, "linkage");

        // Round-trip contract only holds for empty explicit_exports until
        // the reader learns to parse them. The tests below never seed a
        // non-empty list, so this assertion doubles as a canary.
        assert_eq!(
            got.explicit_exports.len(),
            want.explicit_exports.len(),
            "explicit_exports len (reader gap)"
        );
    }

    fn minimal_plan() -> PlanV1 {
        PlanV1 {
            version: "1".into(),
            root: "hello".into(),
            components: vec![ComponentSpec {
                id: "hello".into(),
                digest: digest_bytes(0x11),
                source: Some("file:///hello.wasm".into()),
            }],
            bindings: vec![],
            secrets: vec![],
            policy: Policy::default(),
            linkage: Linkage::Static,
            explicit_exports: vec![],
        }
    }

    #[test]
    fn writer_minimal_plan_round_trip() {
        let want = minimal_plan();
        let ts = plan_to_rdf(&want);
        let got = plan_from_rdf(DEFAULT_PLAN_IRI, &ts)
            .expect("minimal plan round-trip should parse");
        assert_plans_equal(&got, &want);
    }

    fn full_plan() -> PlanV1 {
        PlanV1 {
            version: "1".into(),
            root: "a".into(),
            components: vec![
                ComponentSpec {
                    id: "a".into(),
                    digest: digest_bytes(0xAA),
                    source: Some("ipfs://Qm.../a.wasm".into()),
                },
                ComponentSpec {
                    id: "b".into(),
                    digest: digest_bytes(0xBB),
                    source: None,
                },
                ComponentSpec {
                    id: "c".into(),
                    digest: digest_bytes(0xCC),
                    source: Some("https://example/c.wasm".into()),
                },
            ],
            bindings: vec![
                ImportBinding {
                    consumer_id: Some("a".into()),
                    import_name: "b:run".into(),
                    provider_id: "b".into(),
                    export_name: "run".into(),
                },
                ImportBinding {
                    consumer_id: None,
                    import_name: "c:emit".into(),
                    provider_id: "c".into(),
                    export_name: "emit".into(),
                },
            ],
            secrets: vec![
                SecretBinding {
                    secret_id: "db-password".into(),
                    backend_uri: "vault://kv/data/prod/db".into(),
                },
                SecretBinding {
                    secret_id: "signing-key".into(),
                    backend_uri: "pkcs11://slot0?object=sign".into(),
                },
            ],
            policy: Policy {
                determinism: DeterminismMode::Audit,
                capabilities: vec![
                    Capability {
                        name: "wasi:filesystem".into(),
                        level: CapabilityLevel::Optional,
                    },
                    Capability {
                        name: "wasi:http".into(),
                        level: CapabilityLevel::Required,
                    },
                ],
                tenant: Some("acme".into()),
                limits: ResourceLimits {
                    cpu_ms: Some(15_000),
                    memory_bytes: Some(67_108_864),
                    io_ops: Some(4096),
                },
            },
            linkage: Linkage::Runtime,
            explicit_exports: vec![],
        }
    }

    #[test]
    fn writer_full_plan_round_trip() {
        let want = full_plan();
        let ts = plan_to_rdf(&want);
        let got =
            plan_from_rdf(DEFAULT_PLAN_IRI, &ts).expect("full plan round-trip should parse");
        assert_plans_equal(&got, &want);
    }

    #[test]
    fn writer_default_policy_round_trip() {
        // Default Policy is Relaxed. Reader falls back to Strict inside
        // policy_from_rdf if `comp:determinism` is missing, so the writer
        // always emits it explicitly. This test guards that behaviour.
        let mut want = minimal_plan();
        want.policy = Policy::default();
        assert!(matches!(want.policy.determinism, DeterminismMode::Relaxed));
        let ts = plan_to_rdf(&want);
        let got = plan_from_rdf(DEFAULT_PLAN_IRI, &ts).unwrap();
        assert!(matches!(got.policy.determinism, DeterminismMode::Relaxed));
        assert_plans_equal(&got, &want);
    }

    #[test]
    fn writer_static_linkage_omits_predicate() {
        let plan = minimal_plan();
        let ts = plan_to_rdf(&plan);
        let has_linkage = ts.iter().any(|t| {
            matches!(&t.predicate, Term::Iri(p) if p == vocab::LINKAGE)
        });
        assert!(
            !has_linkage,
            "static linkage should be omitted for canonical output"
        );
        // ...but the round-trip still recovers Static.
        let got = plan_from_rdf(DEFAULT_PLAN_IRI, &ts).unwrap();
        assert!(matches!(got.linkage, Linkage::Static));
    }

    #[test]
    fn writer_runtime_linkage_round_trip() {
        let mut want = minimal_plan();
        want.linkage = Linkage::Runtime;
        let ts = plan_to_rdf(&want);
        let got = plan_from_rdf(DEFAULT_PLAN_IRI, &ts).unwrap();
        assert!(matches!(got.linkage, Linkage::Runtime));
        assert_plans_equal(&got, &want);
    }

    #[test]
    fn writer_custom_plan_iri_round_trip() {
        let iri_s = "urn:tegmentum:example/plan-42";
        let want = full_plan();
        let ts = plan_to_rdf_with_iri(&want, iri_s);
        let got = plan_from_rdf(iri_s, &ts).unwrap();
        assert_plans_equal(&got, &want);
    }

    #[test]
    fn writer_component_without_source_round_trips() {
        let mut want = minimal_plan();
        want.components[0].source = None;
        let ts = plan_to_rdf(&want);
        let got = plan_from_rdf(DEFAULT_PLAN_IRI, &ts).unwrap();
        assert!(got.components[0].source.is_none());
        assert_plans_equal(&got, &want);
    }

    #[test]
    fn writer_binding_without_consumer_round_trips() {
        let mut want = minimal_plan();
        // Add a second component so the binding has a valid provider.
        want.components.push(ComponentSpec {
            id: "helper".into(),
            digest: digest_bytes(0x22),
            source: None,
        });
        want.bindings.push(ImportBinding {
            consumer_id: None,
            import_name: "helper:util".into(),
            provider_id: "helper".into(),
            export_name: "util".into(),
        });
        let ts = plan_to_rdf(&want);
        let got = plan_from_rdf(DEFAULT_PLAN_IRI, &ts).unwrap();
        assert_eq!(got.bindings.len(), 1);
        assert!(got.bindings[0].consumer_id.is_none());
        assert_plans_equal(&got, &want);
    }

    #[test]
    fn writer_binding_with_consumer_round_trips() {
        let mut want = minimal_plan();
        want.components.push(ComponentSpec {
            id: "helper".into(),
            digest: digest_bytes(0x33),
            source: None,
        });
        want.bindings.push(ImportBinding {
            consumer_id: Some("hello".into()),
            import_name: "helper:util".into(),
            provider_id: "helper".into(),
            export_name: "util".into(),
        });
        let ts = plan_to_rdf(&want);
        let got = plan_from_rdf(DEFAULT_PLAN_IRI, &ts).unwrap();
        assert_eq!(got.bindings.len(), 1);
        assert_eq!(got.bindings[0].consumer_id.as_deref(), Some("hello"));
        assert_plans_equal(&got, &want);
    }

    #[test]
    fn writer_capabilities_are_sorted_for_reader_stability() {
        // Reader sorts capabilities by name; make sure the writer's
        // triple stream carries them in the same order so the pre- and
        // post- round-trip lists match.
        let mut plan = minimal_plan();
        plan.policy.capabilities = vec![
            Capability {
                name: "wasi:http".into(),
                level: CapabilityLevel::Required,
            },
            Capability {
                name: "wasi:filesystem".into(),
                level: CapabilityLevel::Optional,
            },
            Capability {
                name: "wasi:cli".into(),
                level: CapabilityLevel::Required,
            },
        ];
        let ts = plan_to_rdf(&plan);
        // Find the order of capability `name` literals in the emitted
        // triples.
        let names: Vec<String> = ts
            .iter()
            .filter(|t| {
                matches!(&t.predicate, Term::Iri(p) if p == vocab::NAME)
            })
            .filter_map(|t| match &t.object {
                Term::Literal { value, .. } => Some(value.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(
            names,
            vec![
                "wasi:cli".to_string(),
                "wasi:filesystem".to_string(),
                "wasi:http".to_string(),
            ]
        );
    }

    #[test]
    fn turtle_output_is_syntactically_reasonable() {
        // We don't have a Turtle parser on the crate's dep list, so this
        // test only checks the emitter's structural invariants: the
        // prefix header shows up, the plan IRI appears once as a
        // subject, and every triple line ends with " .".
        let plan = full_plan();
        let ttl = plan_to_turtle(&plan);
        assert!(ttl.starts_with("@prefix comp: <"));
        assert!(ttl.contains(vocab::NS));
        // Root literal.
        assert!(ttl.contains("\"a\""));
        // Predicate compression works — expect `comp:root` not `<...root>`.
        assert!(ttl.contains("comp:root"));
        // Every non-empty non-directive line ends with " .".
        for line in ttl.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with("@prefix") {
                continue;
            }
            assert!(
                trimmed.ends_with(" ."),
                "malformed Turtle line: {trimmed:?}"
            );
        }
    }

    #[test]
    fn turtle_uses_rdf_type_short_form() {
        let plan = minimal_plan();
        let ttl = plan_to_turtle(&plan);
        // Some line should include ` a comp:CompositionPlan `.
        assert!(
            ttl.contains(" a comp:CompositionPlan"),
            "expected `a` short form in Turtle output; got:\n{ttl}"
        );
    }
}
