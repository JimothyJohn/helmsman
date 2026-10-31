//! The address space: the standard's skeleton (Root, Objects, Types,
//! Views, and the Server object a client checks on), then this box under
//! Objects:
//!
//! ```text
//! Objects
//! ├── Signals                       ns=1;s=signals
//! │   └── <signal>                  ns=1;s=signals/<signal>          value, quality, timestamp
//! │       └── EngineeringUnits      ns=1;s=signals/<signal>/EngineeringUnits   (with unit=)
//! └── Modules                       ns=1;s=modules
//!     └── <module>                  ns=1;s=modules/<module>
//!         ├── Status                ns=1;s=modules/<module>/Status   running, held, faulted: why
//!         └── Outputs               ns=1;s=modules/<module>/Outputs
//!             └── <output>          ns=1;s=modules/<module>/Outputs/<output>
//!                 └── EngineeringUnits
//! ```
//!
//! Every variable is read-only (`AccessLevel` `CurrentRead`, `WriteMask`
//! 0). Built from a [`Shape`] on a server thread, never the scan thread;
//! values come from the [`Snapshot`] at the moment of each read.

use std::collections::HashMap;

use helmsman_io::{Dir, Ty, Value};

use super::codec::{DataValue, NodeId, Variant, Writer, date_time};
use super::feed::{Shape, Snapshot};
use super::status;

/// This box's namespace URI (index 1).
pub const NAMESPACE: &str = "urn:helmsman:plc";
/// The server's application URI.
pub const APP_URI: &str = "urn:helmsman:plc:server";
/// `EUInformation.NamespaceUri`: UNECE Recommendation 20 codes.
const UNITS_NS: &str = "http://www.opcfoundation.org/UA/units/un/cefact";

/// Standard node, reference and type ids used here (Part 6 `NodeIds.csv`).
pub mod ids {
    pub const ROOT: u32 = 84;
    pub const OBJECTS: u32 = 85;
    pub const TYPES: u32 = 86;
    pub const VIEWS: u32 = 87;
    pub const SERVER: u32 = 2253;
    pub const SERVER_ARRAY: u32 = 2254;
    pub const NAMESPACE_ARRAY: u32 = 2255;
    pub const SERVER_STATUS: u32 = 2256;
    pub const START_TIME: u32 = 2257;
    pub const CURRENT_TIME: u32 = 2258;
    pub const STATE: u32 = 2259;

    pub const FOLDER_TYPE: u32 = 61;
    pub const BASE_DATA_VARIABLE_TYPE: u32 = 63;
    pub const PROPERTY_TYPE: u32 = 68;
    pub const SERVER_TYPE: u32 = 2004;
    pub const SERVER_STATUS_TYPE: u32 = 2138;

    pub const REFERENCES: u32 = 31;
    pub const NON_HIERARCHICAL: u32 = 32;
    pub const HIERARCHICAL: u32 = 33;
    pub const HAS_CHILD: u32 = 34;
    pub const ORGANIZES: u32 = 35;
    pub const HAS_EVENT_SOURCE: u32 = 36;
    pub const HAS_MODELLING_RULE: u32 = 37;
    pub const HAS_ENCODING: u32 = 38;
    pub const HAS_DESCRIPTION: u32 = 39;
    pub const HAS_TYPE_DEFINITION: u32 = 40;
    pub const GENERATES_EVENT: u32 = 41;
    pub const AGGREGATES: u32 = 44;
    pub const HAS_SUBTYPE: u32 = 45;
    pub const HAS_PROPERTY: u32 = 46;
    pub const HAS_COMPONENT: u32 = 47;
    pub const HAS_NOTIFIER: u32 = 48;
    pub const HAS_ORDERED_COMPONENT: u32 = 49;

    pub const BOOLEAN: u32 = 1;
    pub const INT32: u32 = 6;
    pub const INT64: u32 = 8;
    pub const DOUBLE: u32 = 11;
    pub const STRING: u32 = 12;
    pub const UTC_TIME: u32 = 294;
    pub const EU_INFORMATION: u32 = 887;
    pub const EU_INFORMATION_BINARY: u32 = 889;
    pub const SERVER_STATE: u32 = 852;
    pub const SERVER_STATUS_DATA_TYPE: u32 = 862;
    pub const SERVER_STATUS_BINARY: u32 = 864;
}

/// The reference types this server knows, each with its supertype.
const REF_TYPES: &[(u32, u32)] = &[
    (ids::REFERENCES, 0),
    (ids::HIERARCHICAL, ids::REFERENCES),
    (ids::NON_HIERARCHICAL, ids::REFERENCES),
    (ids::HAS_CHILD, ids::HIERARCHICAL),
    (ids::ORGANIZES, ids::HIERARCHICAL),
    (ids::HAS_EVENT_SOURCE, ids::HIERARCHICAL),
    (ids::AGGREGATES, ids::HAS_CHILD),
    (ids::HAS_SUBTYPE, ids::HAS_CHILD),
    (ids::HAS_PROPERTY, ids::AGGREGATES),
    (ids::HAS_COMPONENT, ids::AGGREGATES),
    (ids::HAS_ORDERED_COMPONENT, ids::HAS_COMPONENT),
    (ids::HAS_NOTIFIER, ids::HAS_EVENT_SOURCE),
    (ids::HAS_TYPE_DEFINITION, ids::NON_HIERARCHICAL),
    (ids::HAS_MODELLING_RULE, ids::NON_HIERARCHICAL),
    (ids::HAS_ENCODING, ids::NON_HIERARCHICAL),
    (ids::HAS_DESCRIPTION, ids::NON_HIERARCHICAL),
    (ids::GENERATES_EVENT, ids::NON_HIERARCHICAL),
];

/// Whether `id` is a reference type this server knows.
#[must_use]
pub fn known_ref_type(id: u32) -> bool {
    REF_TYPES.iter().any(|(t, _)| *t == id)
}

/// Whether reference type `ty` is `filter` or (with `subtypes`) below it.
#[must_use]
pub fn ref_matches(ty: u32, filter: u32, subtypes: bool) -> bool {
    if ty == filter {
        return true;
    }
    if !subtypes {
        return false;
    }
    let mut t = ty;
    for _ in 0..REF_TYPES.len() {
        match REF_TYPES.iter().find(|(id, _)| *id == t) {
            Some((_, 0)) | None => return false,
            Some((_, parent)) if *parent == filter => return true,
            Some((_, parent)) => t = *parent,
        }
    }
    false
}

/// `NodeClass` values (a bit each, as `nodeClassMask` uses them).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    Object = 1,
    Variable = 2,
    ObjectType = 8,
    VariableType = 16,
}

/// What a variable's value is.
#[derive(Debug, Clone, PartialEq)]
pub enum Source {
    /// A signal's value, by table index.
    Signal(usize, Ty),
    /// A module's state, by scan index.
    ModuleStatus(usize),
    /// An `EUInformation` for this unit.
    Units(String),
    NamespaceArray,
    ServerArray,
    ServerStatus,
    State,
    CurrentTime,
    StartTime,
}

impl Source {
    /// `DataType` and `ValueRank`.
    const fn data_type(&self) -> (u32, i32) {
        match self {
            Self::Signal(_, Ty::Bool) => (ids::BOOLEAN, -1),
            Self::Signal(_, Ty::Int) => (ids::INT64, -1),
            Self::Signal(_, Ty::Real) => (ids::DOUBLE, -1),
            Self::ModuleStatus(_) => (ids::STRING, -1),
            Self::Units(_) => (ids::EU_INFORMATION, -1),
            Self::NamespaceArray | Self::ServerArray => (ids::STRING, 1),
            Self::ServerStatus => (ids::SERVER_STATUS_DATA_TYPE, -1),
            Self::State => (ids::SERVER_STATE, -1),
            Self::CurrentTime | Self::StartTime => (ids::UTC_TIME, -1),
        }
    }
}

/// One reference, as held by its source node (inverse ones too).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ref {
    pub ty: u32,
    pub forward: bool,
    pub target: NodeId,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub id: NodeId,
    pub class: Class,
    pub browse: (u16, String),
    pub display: String,
    pub description: Option<String>,
    pub refs: Vec<Ref>,
    /// Variables only.
    pub source: Option<Source>,
}

impl Node {
    /// The target of this node's `HasTypeDefinition`, if any.
    #[must_use]
    pub fn type_definition(&self) -> Option<&NodeId> {
        self.refs
            .iter()
            .find(|r| r.forward && r.ty == ids::HAS_TYPE_DEFINITION)
            .map(|r| &r.target)
    }
}

/// What the server reports about itself.
#[derive(Debug, Clone)]
pub struct About {
    /// When the server started, Unix ms.
    pub started_ms: i64,
    pub version: String,
}

/// The nodes, by id.
#[derive(Debug, Clone)]
pub struct Space {
    nodes: Vec<Node>,
    index: HashMap<NodeId, usize>,
}

/// Type definition nodes referred to but not served: enough to describe
/// a `HasTypeDefinition` target when browsing.
const TYPE_NODES: &[(u32, &str, Class)] = &[
    (ids::FOLDER_TYPE, "FolderType", Class::ObjectType),
    (ids::SERVER_TYPE, "ServerType", Class::ObjectType),
    (
        ids::BASE_DATA_VARIABLE_TYPE,
        "BaseDataVariableType",
        Class::VariableType,
    ),
    (ids::PROPERTY_TYPE, "PropertyType", Class::VariableType),
    (ids::SERVER_STATUS_TYPE, "ServerStatusType", Class::VariableType),
];

/// How a reference target is described when browsing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetInfo {
    pub browse: (u16, String),
    pub display: String,
    pub class: Class,
    pub type_definition: Option<NodeId>,
}

impl Space {
    /// The address space for `shape`.
    #[must_use]
    pub fn build(shape: &Shape) -> Self {
        let mut b = Builder::default();
        let folder = |b: &mut Builder, id: NodeId, ns: u16, name: &str, desc: Option<String>| {
            b.node(Node {
                id: id.clone(),
                class: Class::Object,
                browse: (ns, name.into()),
                display: name.into(),
                description: desc,
                refs: Vec::new(),
                source: None,
            });
            b.type_def(&id, ids::FOLDER_TYPE);
        };
        let root = NodeId::ns0(ids::ROOT);
        let objects = NodeId::ns0(ids::OBJECTS);
        folder(&mut b, root.clone(), 0, "Root", None);
        folder(&mut b, objects.clone(), 0, "Objects", None);
        folder(&mut b, NodeId::ns0(ids::TYPES), 0, "Types", None);
        folder(&mut b, NodeId::ns0(ids::VIEWS), 0, "Views", None);
        for f in [ids::OBJECTS, ids::TYPES, ids::VIEWS] {
            b.reference(&root, ids::ORGANIZES, &NodeId::ns0(f));
        }
        b.server(&objects);

        let signals = NodeId::box_("signals");
        folder(
            &mut b,
            signals.clone(),
            1,
            "Signals",
            Some("Every signal of the box: inputs as last read, outputs as commanded".into()),
        );
        b.reference(&objects, ids::ORGANIZES, &signals);
        for (i, s) in shape.signals.iter().enumerate() {
            let id = NodeId::box_(format!("signals/{}", s.name));
            let desc = match (s.dir, &s.source) {
                (Dir::In, Some(m)) => format!("output of module {m}"),
                (Dir::In, None) => "input".to_string(),
                (Dir::Out, _) => "output command".to_string(),
            };
            b.variable(
                &signals,
                ids::ORGANIZES,
                id,
                &s.name,
                Some(desc),
                Source::Signal(i, s.ty),
            );
            b.units(s.unit.as_deref());
        }

        let modules = NodeId::box_("modules");
        folder(
            &mut b,
            modules.clone(),
            1,
            "Modules",
            Some("One folder per module: its state and its outputs".into()),
        );
        b.reference(&objects, ids::ORGANIZES, &modules);
        for (m, module) in shape.modules.iter().enumerate() {
            let base = format!("modules/{}", module.name);
            let mid = NodeId::box_(base.clone());
            folder(&mut b, mid.clone(), 1, &module.name, None);
            b.reference(&modules, ids::ORGANIZES, &mid);
            b.variable(
                &mid,
                ids::HAS_COMPONENT,
                NodeId::box_(format!("{base}/Status")),
                "Status",
                Some("running, held (a signal it reads has no value) or faulted: why".into()),
                Source::ModuleStatus(m),
            );
            let outs = NodeId::box_(format!("{base}/Outputs"));
            folder(&mut b, outs.clone(), 1, "Outputs", None);
            b.reference(&mid, ids::ORGANIZES, &outs);
            for (port, i) in &module.outputs {
                let Some(s) = shape.signals.get(*i) else { continue };
                b.variable(
                    &outs,
                    ids::ORGANIZES,
                    NodeId::box_(format!("{base}/Outputs/{port}")),
                    port,
                    Some(format!("signal {}", s.name)),
                    Source::Signal(*i, s.ty),
                );
                b.units(s.unit.as_deref());
            }
        }
        let Builder { nodes, index, .. } = b;
        Self { nodes, index }
    }

    #[must_use]
    pub fn node(&self, id: &NodeId) -> Option<&Node> {
        self.index.get(id).map(|&i| &self.nodes[i])
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// How to describe `id` as a browse target.
    #[must_use]
    pub fn target(&self, id: &NodeId) -> Option<TargetInfo> {
        if let Some(n) = self.node(id) {
            return Some(TargetInfo {
                browse: n.browse.clone(),
                display: n.display.clone(),
                class: n.class,
                type_definition: n.type_definition().cloned(),
            });
        }
        let t = id.ns0_id()?;
        TYPE_NODES
            .iter()
            .find(|(i, ..)| *i == t)
            .map(|(_, name, class)| TargetInfo {
                browse: (0, (*name).to_string()),
                display: (*name).to_string(),
                class: *class,
                type_definition: None,
            })
    }
}

#[derive(Default)]
struct Builder {
    nodes: Vec<Node>,
    index: HashMap<NodeId, usize>,
    /// The last variable added, for [`Builder::units`].
    last: Option<NodeId>,
}

impl Builder {
    fn node(&mut self, n: Node) {
        // Names are unique in the table (case-insensitively), so ids are;
        // a duplicate would only shadow, never panic.
        if self.index.contains_key(&n.id) {
            return;
        }
        self.index.insert(n.id.clone(), self.nodes.len());
        self.nodes.push(n);
    }

    fn get(&mut self, id: &NodeId) -> Option<&mut Node> {
        self.index.get(id).map(|&i| &mut self.nodes[i])
    }

    /// `from` → `to`, and the inverse on `to`.
    fn reference(&mut self, from: &NodeId, ty: u32, to: &NodeId) {
        if let Some(n) = self.get(from) {
            n.refs.push(Ref {
                ty,
                forward: true,
                target: to.clone(),
            });
        }
        if let Some(n) = self.get(to) {
            n.refs.push(Ref {
                ty,
                forward: false,
                target: from.clone(),
            });
        }
    }

    fn type_def(&mut self, id: &NodeId, ty: u32) {
        if let Some(n) = self.get(id) {
            n.refs.push(Ref {
                ty: ids::HAS_TYPE_DEFINITION,
                forward: true,
                target: NodeId::ns0(ty),
            });
        }
    }

    fn variable(
        &mut self,
        parent: &NodeId,
        via: u32,
        id: NodeId,
        name: &str,
        description: Option<String>,
        source: Source,
    ) {
        let ns = match &id {
            NodeId::Num(ns, _) | NodeId::Str(ns, _) | NodeId::Guid(ns, _) | NodeId::Bytes(ns, _) => *ns,
        };
        let property = via == ids::HAS_PROPERTY;
        self.node(Node {
            id: id.clone(),
            class: Class::Variable,
            browse: (ns, name.into()),
            display: name.into(),
            description,
            refs: Vec::new(),
            source: Some(source),
        });
        self.reference(parent, via, &id);
        self.type_def(
            &id,
            if property {
                ids::PROPERTY_TYPE
            } else {
                ids::BASE_DATA_VARIABLE_TYPE
            },
        );
        self.last = Some(id);
    }

    /// An `EngineeringUnits` property on the last variable added.
    fn units(&mut self, unit: Option<&str>) {
        let (Some(unit), Some(var)) = (unit, self.last.clone()) else {
            return;
        };
        let NodeId::Str(_, base) = &var else { return };
        self.variable(
            &var,
            ids::HAS_PROPERTY,
            NodeId::box_(format!("{base}/EngineeringUnits")),
            "EngineeringUnits",
            None,
            Source::Units(unit.to_string()),
        );
        // `EngineeringUnits` is the standard's browse name (namespace 0).
        if let Some(n) = self.get(&NodeId::box_(format!("{base}/EngineeringUnits"))) {
            n.browse.0 = 0;
        }
    }

    /// The `Server` object and the parts of it a client reads on connect.
    fn server(&mut self, objects: &NodeId) {
        let server = NodeId::ns0(ids::SERVER);
        self.node(Node {
            id: server.clone(),
            class: Class::Object,
            browse: (0, "Server".into()),
            display: "Server".into(),
            description: None,
            refs: Vec::new(),
            source: None,
        });
        self.type_def(&server, ids::SERVER_TYPE);
        self.reference(objects, ids::ORGANIZES, &server);
        self.variable(
            &server,
            ids::HAS_PROPERTY,
            NodeId::ns0(ids::SERVER_ARRAY),
            "ServerArray",
            None,
            Source::ServerArray,
        );
        self.variable(
            &server,
            ids::HAS_PROPERTY,
            NodeId::ns0(ids::NAMESPACE_ARRAY),
            "NamespaceArray",
            None,
            Source::NamespaceArray,
        );
        let status = NodeId::ns0(ids::SERVER_STATUS);
        self.variable(
            &server,
            ids::HAS_COMPONENT,
            status.clone(),
            "ServerStatus",
            None,
            Source::ServerStatus,
        );
        if let Some(n) = self.get(&status) {
            n.refs.retain(|r| r.ty != ids::HAS_TYPE_DEFINITION);
        }
        self.type_def(&status, ids::SERVER_STATUS_TYPE);
        for (id, name, src) in [
            (ids::START_TIME, "StartTime", Source::StartTime),
            (ids::CURRENT_TIME, "CurrentTime", Source::CurrentTime),
            (ids::STATE, "State", Source::State),
        ] {
            self.variable(&status, ids::HAS_COMPONENT, NodeId::ns0(id), name, None, src);
        }
    }
}

/// Attribute ids (Part 4 §5.9 / Part 6 A.1).
pub mod attr {
    pub const NODE_ID: u32 = 1;
    pub const NODE_CLASS: u32 = 2;
    pub const BROWSE_NAME: u32 = 3;
    pub const DISPLAY_NAME: u32 = 4;
    pub const DESCRIPTION: u32 = 5;
    pub const WRITE_MASK: u32 = 6;
    pub const USER_WRITE_MASK: u32 = 7;
    pub const EVENT_NOTIFIER: u32 = 12;
    pub const VALUE: u32 = 13;
    pub const DATA_TYPE: u32 = 14;
    pub const VALUE_RANK: u32 = 15;
    pub const ARRAY_DIMENSIONS: u32 = 16;
    pub const ACCESS_LEVEL: u32 = 17;
    pub const USER_ACCESS_LEVEL: u32 = 18;
    pub const MINIMUM_SAMPLING_INTERVAL: u32 = 19;
    pub const HISTORIZING: u32 = 20;
}

/// `AccessLevel`: `CurrentRead` only.
const READ_ONLY: u8 = 1;

/// Which timestamps a read returns (`TimestampsToReturn`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stamps {
    Source,
    Server,
    Both,
    Neither,
}

impl Stamps {
    #[must_use]
    pub const fn from_u32(v: u32) -> Option<Self> {
        match v {
            0 => Some(Self::Source),
            1 => Some(Self::Server),
            2 => Some(Self::Both),
            3 => Some(Self::Neither),
            _ => None,
        }
    }

    const fn source(self) -> bool {
        matches!(self, Self::Source | Self::Both)
    }

    const fn server(self) -> bool {
        matches!(self, Self::Server | Self::Both)
    }
}

/// What a read needs besides the node.
#[derive(Debug, Clone, Copy)]
pub struct ReadCtx<'a> {
    pub snap: &'a Snapshot,
    pub about: &'a About,
    /// The wall clock now, Unix ms (trusted or not: the server's own
    /// timestamps always carry it).
    pub now_ms: i64,
    /// The publication rate, the fastest a value can change.
    pub sampling_ms: f64,
}

fn value_of(v: Value) -> Variant {
    match v {
        Value::Bool(b) => Variant::Bool(b),
        Value::Int(n) => Variant::I64(n),
        Value::Real(x) => Variant::F64(x),
    }
}

/// `EUInformation` for `unit`: the unit as its display name; no UNECE code
/// (`UnitId` -1, "unknown"), since the map's units are free text.
fn eu_information(unit: &str) -> Vec<u8> {
    let mut w = Writer::new();
    w.string(Some(UNITS_NS))
        .i32(-1)
        .localized_text(Some(unit))
        .localized_text(Some(unit));
    w.buf
}

fn server_status(c: &ReadCtx<'_>) -> Vec<u8> {
    let mut w = Writer::new();
    w.i64(date_time(c.about.started_ms))
        .i64(date_time(c.now_ms))
        .i32(0)
        // BuildInfo
        .string(Some("urn:helmsman"))
        .string(Some("Helmsman"))
        .string(Some("Helmsman PLC"))
        .string(Some(&c.about.version))
        .string(Some(&c.about.version))
        .i64(date_time(c.about.started_ms))
        .u32(0)
        .localized_text(None);
    w.buf
}

/// The `Value` attribute of a variable with `src`: value, quality, and the
/// timestamps asked for.
#[must_use]
pub fn value(src: &Source, c: &ReadCtx<'_>, stamps: Stamps) -> DataValue {
    let now = date_time(c.now_ms);
    let mut dv = DataValue {
        server_time: stamps.server().then_some(now),
        ..DataValue::default()
    };
    match src {
        Source::Signal(i, _) => {
            let shape = c.snap.shape.signals.get(*i);
            match c.snap.values.get(*i).copied().flatten() {
                Some(v) => {
                    dv.value = Some(value_of(v));
                    if c.snap.errors.get(*i).copied().unwrap_or(false) {
                        dv.status = status::UNCERTAIN;
                    }
                }
                None if shape.is_some_and(|s| s.device) => dv.status = status::BAD_NO_COMMUNICATION,
                None if c.snap.values.len() <= *i => dv.status = status::BAD_WAITING_FOR_INITIAL_DATA,
                None => dv.status = status::BAD_OUT_OF_SERVICE,
            }
            if stamps.source() {
                dv.source_time = c.snap.changed.get(*i).copied().flatten().map(date_time);
            }
        }
        Source::ModuleStatus(m) => match c.snap.modules.get(*m) {
            Some(s) => dv.value = Some(Variant::Str(s.text())),
            None => dv.status = status::BAD_WAITING_FOR_INITIAL_DATA,
        },
        Source::Units(u) => {
            dv.value = Some(Variant::Ext(
                NodeId::ns0(ids::EU_INFORMATION_BINARY),
                eu_information(u),
            ));
        }
        Source::NamespaceArray => {
            dv.value = Some(Variant::StrArray(vec![
                "http://opcfoundation.org/UA/".into(),
                NAMESPACE.into(),
            ]));
        }
        Source::ServerArray => dv.value = Some(Variant::StrArray(vec![APP_URI.into()])),
        Source::ServerStatus => {
            dv.value = Some(Variant::Ext(
                NodeId::ns0(ids::SERVER_STATUS_BINARY),
                server_status(c),
            ));
        }
        // ServerState Running.
        Source::State => dv.value = Some(Variant::I32(0)),
        Source::CurrentTime => dv.value = Some(Variant::Time(now)),
        Source::StartTime => dv.value = Some(Variant::Time(date_time(c.about.started_ms))),
    }
    if !matches!(src, Source::Signal(..)) && stamps.source() {
        dv.source_time = Some(now);
    }
    dv
}

/// One attribute of `node`, or the status that says why not.
#[must_use]
pub fn read(node: &Node, attribute: u32, c: &ReadCtx<'_>, stamps: Stamps) -> DataValue {
    let ok = |v: Variant| DataValue {
        value: Some(v),
        ..DataValue::default()
    };
    let var = node.source.as_ref();
    match (attribute, var) {
        (attr::NODE_ID, _) => ok(Variant::Node(node.id.clone())),
        (attr::NODE_CLASS, _) => ok(Variant::I32(node.class as i32)),
        (attr::BROWSE_NAME, _) => ok(Variant::QName(node.browse.0, node.browse.1.clone())),
        (attr::DISPLAY_NAME, _) => ok(Variant::Text(node.display.clone())),
        (attr::DESCRIPTION, _) => match &node.description {
            Some(d) => ok(Variant::Text(d.clone())),
            None => ok(Variant::Text(String::new())),
        },
        (attr::WRITE_MASK | attr::USER_WRITE_MASK, _) => ok(Variant::U32(0)),
        (attr::EVENT_NOTIFIER, None) => ok(Variant::Byte(0)),
        (attr::VALUE, Some(s)) => value(s, c, stamps),
        (attr::DATA_TYPE, Some(s)) => ok(Variant::Node(NodeId::ns0(s.data_type().0))),
        (attr::VALUE_RANK, Some(s)) => ok(Variant::I32(s.data_type().1)),
        (attr::ARRAY_DIMENSIONS, Some(s)) => {
            if s.data_type().1 == 1 {
                ok(Variant::U32Array(vec![0]))
            } else {
                ok(Variant::U32Array(Vec::new()))
            }
        }
        (attr::ACCESS_LEVEL | attr::USER_ACCESS_LEVEL, Some(_)) => ok(Variant::Byte(READ_ONLY)),
        (attr::MINIMUM_SAMPLING_INTERVAL, Some(_)) => ok(Variant::F64(c.sampling_ms)),
        (attr::HISTORIZING, Some(_)) => ok(Variant::Bool(false)),
        _ => DataValue {
            status: status::BAD_ATTRIBUTE_ID_INVALID,
            ..DataValue::default()
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::opcua::feed::{ModShape, SigShape};

    fn shape() -> Shape {
        Shape {
            signals: vec![
                SigShape {
                    name: "level".into(),
                    ty: Ty::Real,
                    dir: Dir::In,
                    unit: Some("mm".into()),
                    source: None,
                    device: true,
                },
                SigShape {
                    name: "pump".into(),
                    ty: Ty::Bool,
                    dir: Dir::Out,
                    unit: None,
                    source: None,
                    device: true,
                },
            ],
            modules: vec![ModShape {
                name: "fill".into(),
                outputs: vec![("pump".into(), 1)],
            }],
        }
    }

    #[test]
    fn signals_and_module_outputs_are_variables_under_their_folders() {
        let s = Space::build(&shape());
        let level = s.node(&NodeId::box_("signals/level")).unwrap();
        assert_eq!(level.class, Class::Variable);
        assert!(
            level
                .refs
                .iter()
                .any(|r| !r.forward && r.target == NodeId::box_("signals"))
        );
        assert!(s.node(&NodeId::box_("signals/level/EngineeringUnits")).is_some());
        assert!(s.node(&NodeId::box_("signals/pump/EngineeringUnits")).is_none());
        assert!(s.node(&NodeId::box_("modules/fill/Outputs/pump")).is_some());
        assert!(s.node(&NodeId::box_("modules/fill/Status")).is_some());
        assert!(s.node(&NodeId::ns0(ids::STATE)).is_some());
    }

    #[test]
    fn subtypes_follow_the_standard_hierarchy() {
        assert!(ref_matches(ids::HAS_PROPERTY, ids::HIERARCHICAL, true));
        assert!(ref_matches(ids::ORGANIZES, ids::REFERENCES, true));
        assert!(!ref_matches(ids::HAS_TYPE_DEFINITION, ids::HIERARCHICAL, true));
        assert!(!ref_matches(ids::HAS_PROPERTY, ids::HIERARCHICAL, false));
    }

    #[test]
    fn a_missing_value_is_bad_and_carries_no_value() {
        let shape = Arc::new(shape());
        let snap = Snapshot {
            seq: 1,
            shape,
            values: vec![None, Some(Value::Bool(true))],
            errors: vec![false, true],
            changed: vec![Some(1), Some(2)],
            modules: Vec::new(),
        };
        let about = About {
            started_ms: 0,
            version: "t".into(),
        };
        let c = ReadCtx {
            snap: &snap,
            about: &about,
            now_ms: 5,
            sampling_ms: 100.0,
        };
        let missing = value(&Source::Signal(0, Ty::Real), &c, Stamps::Both);
        assert_eq!(missing.status, status::BAD_NO_COMMUNICATION);
        assert_eq!(missing.value, None);
        let refused = value(&Source::Signal(1, Ty::Bool), &c, Stamps::Source);
        assert_eq!(refused.status, status::UNCERTAIN);
        assert_eq!(refused.value, Some(Variant::Bool(true)));
        assert_eq!(refused.source_time, Some(date_time(2)));
        assert_eq!(refused.server_time, None);
    }

    use std::sync::Arc;
}
