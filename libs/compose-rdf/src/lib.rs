//! Read a composition `PlanV1` from RDF triples.
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
use std::collections::{BTreeMap, HashMap};

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
}
