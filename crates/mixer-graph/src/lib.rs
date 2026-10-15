//! # novamix-mixer-graph
//!
//! Pure, allocation-tolerant logic about the mixer topology (never runs on
//! the audio thread):
//!
//! * identifiers and limits for channels and buses;
//! * solo resolution ([`SoloMode`]);
//! * the routing graph *source device → channel → bus → output device*,
//!   including system-level links (WASAPI loopback of an output device,
//!   virtual cable pairs) that can close a feedback loop;
//! * cycle detection (Tarjan SCC), human-readable paths, risk warnings and
//!   minimal "fix route" suggestions.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde::{Deserialize, Serialize};

/// Maximum number of input channels supported by the engine.
pub const MAX_CHANNELS: usize = 8;
/// Maximum number of buses supported by the engine.
pub const MAX_BUSES: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ChannelId(pub u8);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct BusId(pub u8);

impl ChannelId {
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

impl BusId {
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

// ---------------------------------------------------------------------------
// Solo

/// How channel solo affects the buses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SoloMode {
    /// Solo only changes what you hear on monitor buses; the stream and
    /// virtual-mic mixes are never interrupted by soloing (safe default).
    #[default]
    MonitorOnly,
    /// Classic solo-in-place: non-soloed channels are muted everywhere.
    AllBuses,
}

/// Returns whether channel `ch` is audible on a bus after solo resolution.
///
/// `any_solo` is true when at least one channel is soloed.
#[inline]
pub fn solo_allows(mode: SoloMode, any_solo: bool, channel_soloed: bool, bus_is_monitor: bool) -> bool {
    if !any_solo || channel_soloed {
        return true;
    }
    match mode {
        SoloMode::AllBuses => false,
        SoloMode::MonitorOnly => !bus_is_monitor,
    }
}

// ---------------------------------------------------------------------------
// Graph description

/// An audio endpoint referenced by the routing graph.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Endpoint {
    /// Backend device identifier.
    pub device_id: String,
    /// For capture endpoints: true when this is a loopback capture of the
    /// render device `device_id`.
    pub loopback: bool,
}

impl Endpoint {
    pub fn capture(id: impl Into<String>) -> Self {
        Self { device_id: id.into(), loopback: false }
    }
    pub fn loopback(id: impl Into<String>) -> Self {
        Self { device_id: id.into(), loopback: true }
    }
    pub fn render(id: impl Into<String>) -> Self {
        Self { device_id: id.into(), loopback: false }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkKind {
    /// Render device whose output is captured again by loopback capture.
    Loopback,
    /// Virtual cable: audio written to the render side appears on the
    /// capture side.
    VirtualCable,
}

/// A link outside the mixer from a render device to a capture device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemLink {
    pub render_device: String,
    pub capture_device: String,
    pub kind: LinkKind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChannelNode {
    pub id: ChannelId,
    pub name: String,
    pub source: Option<Endpoint>,
    pub is_microphone: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BusNode {
    pub id: BusId,
    pub name: String,
    pub output: Option<String>,
    pub is_monitor: bool,
}

/// Everything needed to analyse a routing configuration.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GraphInput {
    pub channels: Vec<ChannelNode>,
    pub buses: Vec<BusNode>,
    /// Active sends (enabled and not at -inf).
    pub sends: Vec<(ChannelId, BusId)>,
    /// Known external links (virtual cable pairs).
    pub links: Vec<SystemLink>,
    /// Render devices that are loudspeakers (acoustic feedback risk).
    pub speaker_devices: BTreeSet<String>,
    /// Display names for device ids.
    pub device_names: HashMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum Node {
    Capture(String, bool),
    Channel(ChannelId),
    Bus(BusId),
    Render(String),
}

/// A detected feedback loop.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RouteCycle {
    /// Human-readable path, first element repeated at the end.
    pub path: Vec<String>,
    /// Sends that participate in the loop (candidates to break it).
    pub sends: Vec<(ChannelId, BusId)>,
}

impl RouteCycle {
    pub fn describe(&self) -> String {
        self.path.join(" → ")
    }
}

/// Non-fatal routing risks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RouteWarning {
    /// A microphone channel reaches a loudspeaker: acoustic echo/howl risk.
    MicToSpeakers { channel: ChannelId, bus: BusId, device: String },
    /// A microphone is routed to a monitor bus (self-monitoring is on).
    MicMonitoring { channel: ChannelId, bus: BusId },
    /// A bus has sends but no output device.
    BusWithoutOutput { bus: BusId },
    /// A channel with a source is not sent anywhere.
    ChannelNotRouted { channel: ChannelId },
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RouteAnalysis {
    pub cycles: Vec<RouteCycle>,
    pub warnings: Vec<RouteWarning>,
}

impl RouteAnalysis {
    pub fn has_cycles(&self) -> bool {
        !self.cycles.is_empty()
    }
}

struct Graph {
    nodes: Vec<Node>,
    index: BTreeMap<Node, usize>,
    adj: Vec<Vec<usize>>,
}

impl Graph {
    fn node(&mut self, n: Node) -> usize {
        if let Some(&i) = self.index.get(&n) {
            return i;
        }
        let i = self.nodes.len();
        self.nodes.push(n.clone());
        self.index.insert(n, i);
        self.adj.push(Vec::new());
        i
    }

    fn edge(&mut self, a: usize, b: usize) {
        if !self.adj[a].contains(&b) {
            self.adj[a].push(b);
        }
    }
}

fn build(input: &GraphInput) -> Graph {
    let mut g = Graph { nodes: Vec::new(), index: BTreeMap::new(), adj: Vec::new() };
    for ch in &input.channels {
        let c = g.node(Node::Channel(ch.id));
        if let Some(src) = &ch.source {
            let s = g.node(Node::Capture(src.device_id.clone(), src.loopback));
            g.edge(s, c);
        }
    }
    for bus in &input.buses {
        let b = g.node(Node::Bus(bus.id));
        if let Some(out) = &bus.output {
            let r = g.node(Node::Render(out.clone()));
            g.edge(b, r);
        }
    }
    for (ch, bus) in &input.sends {
        let c = g.node(Node::Channel(*ch));
        let b = g.node(Node::Bus(*bus));
        g.edge(c, b);
    }
    // Loopback capture of a render device the mixer writes to.
    let renders: Vec<String> = g.nodes.iter().filter_map(|n| if let Node::Render(id) = n { Some(id.clone()) } else { None }).collect();
    for r in &renders {
        let key = Node::Capture(r.clone(), true);
        if let Some(&cap) = g.index.get(&key) {
            let rn = g.index[&Node::Render(r.clone())];
            g.edge(rn, cap);
        }
    }
    for link in &input.links {
        let rkey = Node::Render(link.render_device.clone());
        let ckey = Node::Capture(link.capture_device.clone(), false);
        if let (Some(&rn), Some(&cn)) = (g.index.get(&rkey), g.index.get(&ckey)) {
            g.edge(rn, cn);
        }
    }
    g
}

/// Tarjan's strongly connected components.
fn sccs(g: &Graph) -> Vec<Vec<usize>> {
    struct St<'a> {
        g: &'a Graph,
        index: Vec<Option<usize>>,
        low: Vec<usize>,
        on: Vec<bool>,
        stack: Vec<usize>,
        next: usize,
        out: Vec<Vec<usize>>,
    }
    fn visit(s: &mut St, v: usize) {
        s.index[v] = Some(s.next);
        s.low[v] = s.next;
        s.next += 1;
        s.stack.push(v);
        s.on[v] = true;
        for &w in &s.g.adj[v] {
            match s.index[w] {
                None => {
                    visit(s, w);
                    s.low[v] = s.low[v].min(s.low[w]);
                }
                Some(iw) if s.on[w] => s.low[v] = s.low[v].min(iw),
                _ => {}
            }
        }
        if Some(s.low[v]) == s.index[v] {
            let mut comp = Vec::new();
            while let Some(w) = s.stack.pop() {
                s.on[w] = false;
                comp.push(w);
                if w == v {
                    break;
                }
            }
            s.out.push(comp);
        }
    }
    let n = g.nodes.len();
    let mut s = St { g, index: vec![None; n], low: vec![0; n], on: vec![false; n], stack: Vec::new(), next: 0, out: Vec::new() };
    for v in 0..n {
        if s.index[v].is_none() {
            visit(&mut s, v);
        }
    }
    s.out
}

fn label(input: &GraphInput, n: &Node) -> String {
    let dev = |id: &str| input.device_names.get(id).cloned().unwrap_or_else(|| id.to_string());
    match n {
        Node::Capture(id, true) => format!("{} (loopback)", dev(id)),
        Node::Capture(id, false) => dev(id),
        Node::Channel(c) => input.channels.iter().find(|x| x.id == *c).map(|x| x.name.clone()).unwrap_or_else(|| format!("Channel {}", c.0 + 1)),
        Node::Bus(b) => input.buses.iter().find(|x| x.id == *b).map(|x| x.name.clone()).unwrap_or_else(|| format!("Bus {}", b.0 + 1)),
        Node::Render(id) => dev(id),
    }
}

/// Finds a cycle inside one SCC (BFS from `start` back to itself).
fn cycle_in(g: &Graph, comp: &[usize]) -> Vec<usize> {
    let set: BTreeSet<usize> = comp.iter().copied().collect();
    let start = *comp.iter().min().unwrap_or(&0);
    let mut prev: BTreeMap<usize, usize> = BTreeMap::new();
    let mut queue = std::collections::VecDeque::new();
    queue.push_back(start);
    while let Some(v) = queue.pop_front() {
        for &w in &g.adj[v] {
            if !set.contains(&w) {
                continue;
            }
            if w == start {
                let mut path = vec![start];
                let mut cur = v;
                let mut rev = vec![v];
                while cur != start {
                    cur = prev[&cur];
                    rev.push(cur);
                }
                rev.pop();
                rev.reverse();
                path.extend(rev);
                path.push(start);
                return path;
            }
            if let std::collections::btree_map::Entry::Vacant(e) = prev.entry(w) {
                e.insert(v);
                queue.push_back(w);
            }
        }
    }
    vec![start, start]
}

/// Analyses a routing configuration.
pub fn analyze(input: &GraphInput) -> RouteAnalysis {
    let g = build(input);
    let mut cycles = Vec::new();
    for comp in sccs(&g) {
        let self_loop = comp.len() == 1 && g.adj[comp[0]].contains(&comp[0]);
        if comp.len() < 2 && !self_loop {
            continue;
        }
        let path = cycle_in(&g, &comp);
        let mut sends = Vec::new();
        for w in path.windows(2) {
            if let (Node::Channel(c), Node::Bus(b)) = (&g.nodes[w[0]], &g.nodes[w[1]]) {
                sends.push((*c, *b));
            }
        }
        cycles.push(RouteCycle { path: path.iter().map(|&i| label(input, &g.nodes[i])).collect(), sends });
    }

    let mut warnings = Vec::new();
    for ch in &input.channels {
        let routed: Vec<BusId> = input.sends.iter().filter(|(c, _)| *c == ch.id).map(|(_, b)| *b).collect();
        if ch.source.is_some() && routed.is_empty() {
            warnings.push(RouteWarning::ChannelNotRouted { channel: ch.id });
        }
        if !ch.is_microphone {
            continue;
        }
        for b in routed {
            let Some(bus) = input.buses.iter().find(|x| x.id == b) else { continue };
            if let Some(out) = &bus.output
                && input.speaker_devices.contains(out)
            {
                warnings.push(RouteWarning::MicToSpeakers { channel: ch.id, bus: b, device: label(input, &Node::Render(out.clone())) });
                continue;
            }
            if bus.is_monitor {
                warnings.push(RouteWarning::MicMonitoring { channel: ch.id, bus: b });
            }
        }
    }
    for bus in &input.buses {
        let used = input.sends.iter().any(|(_, b)| *b == bus.id);
        if used && bus.output.is_none() {
            warnings.push(RouteWarning::BusWithoutOutput { bus: bus.id });
        }
    }
    RouteAnalysis { cycles, warnings }
}

/// Returns the cycle that enabling `send` would create, if any. Used to
/// preview a connection before it is activated.
pub fn would_create_cycle(input: &GraphInput, send: (ChannelId, BusId)) -> Option<RouteCycle> {
    if input.sends.contains(&send) {
        return None;
    }
    let before = analyze(input).cycles.len();
    let mut probe = input.clone();
    probe.sends.push(send);
    let after = analyze(&probe);
    if after.cycles.len() > before { after.cycles.into_iter().find(|c| c.sends.contains(&send)) } else { None }
}

/// Proposes a minimal set of sends to disable so that no feedback loop
/// remains. Sends fed by loopback/virtual sources are preferred because
/// cutting them does not affect the microphone path.
pub fn suggest_fixes(input: &GraphInput) -> Vec<(ChannelId, BusId)> {
    let mut work = input.clone();
    let mut fixes = Vec::new();
    for _ in 0..(MAX_CHANNELS * MAX_BUSES) {
        let analysis = analyze(&work);
        let Some(cycle) = analysis.cycles.first() else { break };
        let pick = cycle.sends.iter().copied().max_by_key(|(c, _)| {
            let ch = work.channels.iter().find(|x| x.id == *c);
            match ch.and_then(|x| x.source.as_ref()) {
                Some(src) if src.loopback => 2,
                Some(_) if !ch.map(|x| x.is_microphone).unwrap_or(false) => 1,
                _ => 0,
            }
        });
        match pick {
            Some(s) => {
                work.sends.retain(|x| *x != s);
                fixes.push(s);
            }
            None => break, // cycle without sends cannot be fixed by routing
        }
    }
    fixes
}

/// Human-readable signal paths from a channel to every output it reaches.
pub fn describe_paths(input: &GraphInput, channel: ChannelId) -> Vec<String> {
    let Some(ch) = input.channels.iter().find(|c| c.id == channel) else { return Vec::new() };
    let src = ch.source.as_ref().map(|s| label(input, &Node::Capture(s.device_id.clone(), s.loopback)));
    let mut out = Vec::new();
    for (c, b) in &input.sends {
        if *c != channel {
            continue;
        }
        let bus_label = label(input, &Node::Bus(*b));
        let dest = input
            .buses
            .iter()
            .find(|x| x.id == *b)
            .and_then(|x| x.output.clone())
            .map(|o| label(input, &Node::Render(o)))
            .unwrap_or_else(|| "—".to_string());
        let mut parts = Vec::new();
        if let Some(s) = &src {
            parts.push(s.clone());
        }
        parts.push(ch.name.clone());
        parts.push(bus_label);
        parts.push(dest);
        out.push(parts.join(" → "));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> GraphInput {
        GraphInput {
            channels: vec![
                ChannelNode { id: ChannelId(0), name: "Mic".into(), source: Some(Endpoint::capture("mic")), is_microphone: true },
                ChannelNode { id: ChannelId(1), name: "Desktop".into(), source: Some(Endpoint::loopback("virt_play")), is_microphone: false },
            ],
            buses: vec![
                BusNode { id: BusId(0), name: "Monitor".into(), output: Some("phones".into()), is_monitor: true },
                BusNode { id: BusId(1), name: "Voice".into(), output: Some("cable_in".into()), is_monitor: false },
            ],
            sends: vec![(ChannelId(0), BusId(1)), (ChannelId(1), BusId(0))],
            links: vec![SystemLink { render_device: "cable_in".into(), capture_device: "cable_out".into(), kind: LinkKind::VirtualCable }],
            speaker_devices: BTreeSet::new(),
            device_names: HashMap::new(),
        }
    }

    #[test]
    fn default_route_has_no_cycles() {
        let a = analyze(&base());
        assert!(a.cycles.is_empty(), "{:?}", a.cycles);
    }

    #[test]
    fn loopback_of_monitor_output_is_a_cycle() {
        let mut g = base();
        // Desktop captures the loopback of the headphones it is sent to.
        g.channels[1].source = Some(Endpoint::loopback("phones"));
        let a = analyze(&g);
        assert_eq!(a.cycles.len(), 1);
        assert_eq!(a.cycles[0].sends, vec![(ChannelId(1), BusId(0))]);
        assert!(a.cycles[0].describe().contains("Desktop"));
        assert_eq!(suggest_fixes(&g), vec![(ChannelId(1), BusId(0))]);
    }

    #[test]
    fn virtual_cable_pair_cycle_is_detected() {
        let mut g = base();
        g.channels.push(ChannelNode { id: ChannelId(2), name: "Aux".into(), source: Some(Endpoint::capture("cable_out")), is_microphone: false });
        g.sends.push((ChannelId(2), BusId(1)));
        let a = analyze(&g);
        assert_eq!(a.cycles.len(), 1);
        let fixes = suggest_fixes(&g);
        assert_eq!(fixes, vec![(ChannelId(2), BusId(1))]);
        let mut fixed = g.clone();
        fixed.sends.retain(|s| !fixes.contains(s));
        assert!(analyze(&fixed).cycles.is_empty());
    }

    #[test]
    fn preview_detects_cycle_before_enabling() {
        let mut g = base();
        g.channels[1].source = Some(Endpoint::loopback("cable_in"));
        g.sends.retain(|s| *s != (ChannelId(1), BusId(0)));
        assert!(analyze(&g).cycles.is_empty());
        let c = would_create_cycle(&g, (ChannelId(1), BusId(1))).expect("cycle");
        assert!(c.sends.contains(&(ChannelId(1), BusId(1))));
        assert!(would_create_cycle(&g, (ChannelId(1), BusId(0))).is_none());
    }

    #[test]
    fn mic_to_speakers_warning() {
        let mut g = base();
        g.speaker_devices.insert("phones".into());
        g.sends.push((ChannelId(0), BusId(0)));
        let a = analyze(&g);
        assert!(a.warnings.iter().any(|w| matches!(w, RouteWarning::MicToSpeakers { .. })));
        g.speaker_devices.clear();
        let a = analyze(&g);
        assert!(a.warnings.iter().any(|w| matches!(w, RouteWarning::MicMonitoring { .. })));
    }

    #[test]
    fn solo_modes() {
        assert!(solo_allows(SoloMode::MonitorOnly, false, false, true));
        assert!(!solo_allows(SoloMode::MonitorOnly, true, false, true));
        assert!(solo_allows(SoloMode::MonitorOnly, true, false, false));
        assert!(!solo_allows(SoloMode::AllBuses, true, false, false));
        assert!(solo_allows(SoloMode::AllBuses, true, true, false));
    }

    #[test]
    fn paths_are_human_readable() {
        let mut g = base();
        g.device_names.insert("mic".into(), "USB Mic".into());
        g.device_names.insert("cable_in".into(), "CABLE Input".into());
        let p = describe_paths(&g, ChannelId(0));
        assert_eq!(p, vec!["USB Mic → Mic → Voice → CABLE Input".to_string()]);
    }
}
