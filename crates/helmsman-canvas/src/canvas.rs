//! The node canvas: pan and zoom, hit-testing, dragging nodes and wires, box
//! selection, the search popup, and drawing nodes with live values.

use std::collections::BTreeSet;

use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, PointerButton, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2,
    epaint::CubicBezierShape,
};
use helmsman_graph::{Inst, Kind, NodeId, Project, Scope, Ty, Value, Wire};

use crate::app::{App, pin_kinds};
use crate::look::{self, Layout, layout};

/// Canvas-to-screen transform for one graph: `screen = origin + offset + world * zoom`.
#[derive(Clone, Copy, Debug)]
pub struct View {
    pub offset: Vec2,
    pub zoom: f32,
    pub origin: Pos2,
    /// Screen position of the canvas centre last frame.
    pub last_center: Pos2,
}

impl Default for View {
    fn default() -> Self {
        Self {
            offset: Vec2::new(40.0, 40.0),
            zoom: 1.0,
            origin: Pos2::ZERO,
            last_center: Pos2::new(400.0, 300.0),
        }
    }
}

impl View {
    pub fn to_screen(self, p: Pos2) -> Pos2 {
        self.origin + self.offset + p.to_vec2() * self.zoom
    }

    pub fn to_world(self, s: Pos2) -> Pos2 {
        ((s - self.origin - self.offset) / self.zoom).to_pos2()
    }

    fn rect(&self, r: Rect) -> Rect {
        Rect::from_min_max(self.to_screen(r.min), self.to_screen(r.max))
    }

    fn zoom_at(&mut self, screen: Pos2, factor: f32) {
        let w = self.to_world(screen);
        self.zoom = (self.zoom * factor).clamp(0.15, 3.0);
        self.offset = screen - self.origin - w.to_vec2() * self.zoom;
    }

    /// Frames every node with a margin.
    fn fit(&mut self, area: Rect, layouts: &[(NodeId, Layout)]) {
        let Some(bb) = layouts
            .iter()
            .map(|(_, l)| l.rect)
            .reduce(eframe::egui::Rect::union)
        else {
            *self = Self {
                origin: area.min,
                ..Self::default()
            };
            return;
        };
        let bb = bb.expand(40.0);
        self.zoom = (area.width() / bb.width())
            .min(area.height() / bb.height())
            .clamp(0.15, 1.25);
        self.origin = area.min;
        self.offset = area.center() - area.min - bb.center().to_vec2() * self.zoom;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PortRef {
    pub node: NodeId,
    pub key: u32,
    pub out: bool,
    pub ty: Ty,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Hit {
    Port(PortRef, Pos2),
    Header(NodeId),
    Widget(NodeId),
    Body(NodeId),
    Nothing,
}

#[derive(Clone, Debug, Default)]
enum Drag {
    #[default]
    Idle,
    Pan,
    Nodes {
        before: Box<Project>,
    },
    Wire {
        anchor: PortRef,
        at: Pos2,
        /// Set when an existing wire was picked up, for undo.
        before: Option<Box<Project>>,
    },
    Boxing {
        start: Pos2,
        additive: bool,
    },
}

#[derive(Clone, Debug)]
struct Search {
    screen: Pos2,
    world: Pos2,
    query: String,
    pending: Option<PortRef>,
    focus: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Interaction {
    drag: Drag,
    search: Option<Search>,
}

impl Interaction {
    pub fn busy(&self) -> bool {
        !matches!(self.drag, Drag::Idle) || self.search.is_some()
    }
}

fn hit_test(layouts: &[(NodeId, Layout)], world: Pos2, zoom: f32) -> Hit {
    let r = (PORT_HIT / zoom).max(look::PORT_R);
    for (id, l) in layouts.iter().rev() {
        for (out, ports) in [(false, &l.ins), (true, &l.outs)] {
            for (p, at) in ports {
                if at.distance(world) <= r {
                    return Hit::Port(
                        PortRef {
                            node: *id,
                            key: p.key,
                            out,
                            ty: p.ty,
                        },
                        *at,
                    );
                }
            }
        }
    }
    for (id, l) in layouts.iter().rev() {
        if l.header.contains(world) {
            return Hit::Header(*id);
        }
        if l.widget.is_some_and(|w| w.contains(world)) {
            return Hit::Widget(*id);
        }
        if l.rect.contains(world) {
            return Hit::Body(*id);
        }
    }
    Hit::Nothing
}

/// Screen-space radius for grabbing a port.
const PORT_HIT: f32 = 11.0;

fn bezier(painter: &egui::Painter, a: Pos2, b: Pos2, zoom: f32, color: Color32, width: f32) {
    let dx = ((b.x - a.x).abs() * 0.5).max(40.0 * zoom);
    let shape = CubicBezierShape::from_points_stroke(
        [a, a + Vec2::new(dx, 0.0), b - Vec2::new(dx, 0.0), b],
        false,
        Color32::TRANSPARENT,
        Stroke::new(width * zoom.max(0.5), color),
    );
    painter.add(shape);
}

impl App {
    pub fn canvas(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let scope = self.scope();
        let area = ui.available_rect_before_wrap();
        let response = ui.allocate_rect(area, Sense::click_and_drag());
        let painter = ui.painter_at(area);

        let layouts: Vec<(NodeId, Layout)> = self
            .project
            .graph(scope)
            .map(|g| {
                g.nodes
                    .iter()
                    .map(|(id, n)| (*id, layout(&self.project, n)))
                    .collect()
            })
            .unwrap_or_default();

        let mut view = self.views.get(&scope).copied().unwrap_or_else(|| {
            let mut v = View::default();
            v.fit(area, &layouts);
            v
        });
        // Keep the world fixed on screen if the panel moved or resized.
        view.origin = area.min;
        view.last_center = area.center();

        let (pointer, hover, press_origin, scroll, zoom, space, primary_down) = ctx.input(|i| {
            (
                i.pointer.interact_pos(),
                i.pointer.hover_pos(),
                i.pointer.press_origin(),
                i.smooth_scroll_delta,
                i.zoom_delta(),
                i.key_down(egui::Key::Space),
                i.pointer.primary_down(),
            )
        });

        if response.hovered() {
            if (zoom - 1.0).abs() > f32::EPSILON {
                if let Some(h) = hover {
                    view.zoom_at(h, zoom);
                }
            } else if scroll != Vec2::ZERO && self.ix.search.is_none() {
                view.offset += scroll;
            }
        }

        let frozen = view;
        let world = |s: Pos2| frozen.to_world(s);
        let hit_at = |s: Option<Pos2>| s.map_or(Hit::Nothing, |s| hit_test(&layouts, world(s), frozen.zoom));

        // Momentary buttons: held while the primary button is down on one.
        self.sim.held.clear();
        if primary_down
            && !space
            && let (Hit::Widget(id), Some(path)) = (hit_at(press_origin), self.live_path())
            && self
                .project
                .graph(scope)
                .and_then(|g| g.nodes.get(&id))
                .is_some_and(|n| n.kind == Kind::Button)
        {
            let mut p = path;
            p.push(id);
            self.sim.held.insert(p);
        }

        // Start of a drag.
        if response.drag_started() {
            let pan = response.dragged_by(PointerButton::Secondary)
                || response.dragged_by(PointerButton::Middle)
                || space;
            self.ix.search = None;
            if pan {
                self.ix.drag = Drag::Pan;
            } else if response.dragged_by(PointerButton::Primary) {
                let shift = ctx.input(|i| i.modifiers.shift);
                self.ix.drag = match hit_at(press_origin) {
                    Hit::Port(port, at) if port.out => Drag::Wire {
                        anchor: port,
                        at,
                        before: None,
                    },
                    Hit::Port(port, at) => {
                        // Picking up a wired input moves the wire's far end.
                        let before = Box::new(self.project.clone());
                        match self.project.disconnect(scope, port.node, port.key) {
                            Some(w) => {
                                let from = layouts.iter().find(|(id, _)| *id == w.from).and_then(|(_, l)| {
                                    l.outs
                                        .iter()
                                        .find(|(p, _)| p.key == w.from_port)
                                        .map(|(p, at)| (p.ty, *at))
                                });
                                match from {
                                    Some((ty, at)) => Drag::Wire {
                                        anchor: PortRef {
                                            node: w.from,
                                            key: w.from_port,
                                            out: true,
                                            ty,
                                        },
                                        at,
                                        before: Some(before),
                                    },
                                    None => Drag::Idle,
                                }
                            }
                            None => Drag::Wire {
                                anchor: port,
                                at,
                                before: None,
                            },
                        }
                    }
                    Hit::Header(id) | Hit::Body(id) | Hit::Widget(id) => {
                        if !self.selected.contains(&id) {
                            if !shift {
                                self.selected.clear();
                            }
                            self.selected.insert(id);
                        }
                        Drag::Nodes {
                            before: Box::new(self.project.clone()),
                        }
                    }
                    Hit::Nothing => Drag::Boxing {
                        start: world(press_origin.unwrap_or_default()),
                        additive: shift,
                    },
                };
            }
        }

        // During a drag.
        if response.dragged() {
            let delta = response.drag_delta();
            match &mut self.ix.drag {
                Drag::Pan => view.offset += delta,
                Drag::Nodes { .. } => {
                    let d = delta / view.zoom;
                    if let Some(g) = self.project.graph_mut(scope) {
                        for id in &self.selected {
                            if let Some(n) = g.nodes.get_mut(id) {
                                n.pos[0] += d.x;
                                n.pos[1] += d.y;
                            }
                        }
                    }
                }
                _ => {}
            }
        }

        // End of a drag.
        if response.drag_stopped() {
            match std::mem::take(&mut self.ix.drag) {
                Drag::Nodes { before } => self.commit(*before),
                Drag::Wire { anchor, before, .. } => {
                    let undo_from = before.map_or_else(|| self.project.clone(), |b| *b);
                    match hit_at(pointer) {
                        Hit::Port(target, _) if target.out != anchor.out => {
                            let w = if anchor.out {
                                Wire {
                                    from: anchor.node,
                                    from_port: anchor.key,
                                    to: target.node,
                                    to_port: target.key,
                                }
                            } else {
                                Wire {
                                    from: target.node,
                                    from_port: target.key,
                                    to: anchor.node,
                                    to_port: anchor.key,
                                }
                            };
                            if let Err(e) = self.project.connect(scope, w) {
                                self.toast(&ctx, e.to_string(), true);
                            }
                            self.commit(undo_from);
                        }
                        Hit::Nothing => {
                            self.commit(undo_from);
                            if let Some(p) = pointer {
                                self.ix.search = Some(Search {
                                    screen: p,
                                    world: world(p),
                                    query: String::new(),
                                    pending: Some(anchor),
                                    focus: true,
                                });
                            }
                        }
                        _ => self.commit(undo_from),
                    }
                }
                Drag::Boxing { start, additive } => {
                    if let Some(p) = pointer {
                        let r = Rect::from_two_pos(start, world(p));
                        if !additive {
                            self.selected.clear();
                        }
                        for (id, l) in &layouts {
                            if r.intersects(l.rect) {
                                self.selected.insert(*id);
                            }
                        }
                    }
                }
                Drag::Pan | Drag::Idle => {}
            }
        }

        // Clicks.
        if response.clicked() {
            let shift = ctx.input(|i| i.modifiers.shift);
            self.ix.search = None;
            match hit_at(pointer) {
                Hit::Widget(id) => {
                    let before = self.project.clone();
                    if let Some(n) = self.project.graph_mut(scope).and_then(|g| g.nodes.get_mut(&id))
                        && let Kind::Toggle { on } = &mut n.kind
                    {
                        *on = !*on;
                    }
                    self.commit(before);
                    if !shift {
                        self.selected.clear();
                    }
                    self.selected.insert(id);
                }
                Hit::Header(id) | Hit::Body(id) | Hit::Port(PortRef { node: id, .. }, _) => {
                    if shift {
                        if !self.selected.remove(&id) {
                            self.selected.insert(id);
                        }
                    } else {
                        self.selected = BTreeSet::from([id]);
                    }
                }
                Hit::Nothing => {
                    if !shift {
                        self.selected.clear();
                    }
                }
            }
        }
        if response.double_clicked() {
            match hit_at(pointer) {
                Hit::Header(id) | Hit::Body(id) | Hit::Widget(id) => {
                    let is_block = self
                        .project
                        .graph(scope)
                        .and_then(|g| g.nodes.get(&id))
                        .is_some_and(|n| matches!(n.kind, Kind::Block(_)));
                    if is_block {
                        self.views.insert(scope, view);
                        self.enter(id);
                        ctx.request_repaint();
                        return;
                    }
                }
                Hit::Nothing => {
                    if let Some(p) = pointer {
                        self.ix.search = Some(Search {
                            screen: p,
                            world: world(p),
                            query: String::new(),
                            pending: None,
                            focus: true,
                        });
                    }
                }
                Hit::Port(..) => {}
            }
        }

        // Context menu target is whatever was under the pointer at the click.
        let menu_target = hit_at(press_origin);
        response.context_menu(|ui| self.context_menu(ui, menu_target));

        self.views.insert(scope, view);
        self.draw(&painter, area, &view, &layouts, hover);
        self.search_popup(&ctx);
    }

    fn context_menu(&mut self, ui: &mut egui::Ui, target: Hit) {
        let ctx = ui.ctx().clone();
        let scope = self.scope();
        let id = match target {
            Hit::Header(id) | Hit::Body(id) | Hit::Widget(id) | Hit::Port(PortRef { node: id, .. }, _) => {
                Some(id)
            }
            Hit::Nothing => None,
        };
        if let Some(id) = id {
            if !self.selected.contains(&id) {
                self.selected = BTreeSet::from([id]);
            }
            let is_block = self
                .project
                .graph(scope)
                .and_then(|g| g.nodes.get(&id))
                .is_some_and(|n| matches!(n.kind, Kind::Block(_)));
            if is_block {
                if ui.button("Open").clicked() {
                    self.enter(id);
                    ui.close();
                }
                if ui.button("Explode into parent").clicked() {
                    let before = self.project.clone();
                    if let Ok(ids) = self.project.explode(scope, id) {
                        self.commit(before);
                        self.selected = ids.into_iter().collect();
                    }
                    ui.close();
                }
                if ui.button("Make unique").clicked() {
                    let before = self.project.clone();
                    if self.project.make_unique(scope, id).is_ok() {
                        self.commit(before);
                    }
                    ui.close();
                }
                ui.separator();
            }
            if ui
                .button(format!("Group {} into block", self.selected.len()))
                .clicked()
            {
                self.group_selected(&ctx);
                ui.close();
            }
            if ui.button("Duplicate").clicked() {
                self.duplicate_selected();
                ui.close();
            }
            if ui.button("Copy").clicked() {
                self.copy_selected();
                ui.close();
            }
            if ui.button("Delete").clicked() {
                self.delete_selected();
                ui.close();
            }
        } else {
            if ui.button("Add node…").clicked() {
                if let Some(p) = ui.ctx().input(|i| i.pointer.press_origin()) {
                    let v = self.views.get(&scope).copied().unwrap_or_default();
                    self.ix.search = Some(Search {
                        screen: p,
                        world: v.to_world(p),
                        query: String::new(),
                        pending: None,
                        focus: true,
                    });
                }
                ui.close();
            }
            if ui
                .add_enabled(self.clipboard.is_some(), egui::Button::new("Paste"))
                .clicked()
            {
                self.paste(&ctx);
                ui.close();
            }
            if ui.button("Fit view").clicked() {
                self.views.remove(&scope);
                ui.close();
            }
            if !self.nav.is_empty() && ui.button("Up a level").clicked() {
                self.go_up(1);
                ui.close();
            }
        }
    }

    /// The double-click / drop-a-wire search: type to filter, Enter adds the
    /// first match; with a dangling wire, the new node gets wired to it.
    fn search_popup(&mut self, ctx: &egui::Context) {
        let Some(mut s) = self.ix.search.take() else {
            return;
        };
        let scope = self.scope();
        let mut options: Vec<(String, Kind)> = Kind::palette()
            .into_iter()
            .map(|(cat, k)| (format!("{} · {cat}", k.title(&self.project)), k))
            .collect();
        for (b, d) in &self.project.blocks {
            let ok = match scope {
                Scope::Root => true,
                Scope::Block(s) => *b != s && !self.project.reaches(*b, s),
            };
            if ok {
                options.push((format!("{} · block", d.name), Kind::Block(*b)));
            }
        }
        if scope != Scope::Root {
            options.extend(pin_kinds().into_iter().map(|(l, k)| (l.to_string(), k)));
        }
        if let Some(p) = s.pending {
            // Only offer what can take the dangling wire.
            options.retain(|(_, k)| {
                let (ins, outs) = self.project.ports(k);
                let side = if p.out { ins } else { outs };
                side.iter().any(|port| port.ty == p.ty)
            });
        }
        let q = s.query.to_lowercase();
        options.retain(|(label, _)| q.split_whitespace().all(|w| label.to_lowercase().contains(w)));

        let mut chosen = None;
        let mut close = false;
        egui::Area::new(egui::Id::unique("node-search"))
            .fixed_pos(s.screen)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_width(220.0);
                    let edit = ui.add(egui::TextEdit::singleline(&mut s.query).hint_text("search nodes…"));
                    if s.focus {
                        edit.request_focus();
                        s.focus = false;
                    }
                    if edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        chosen = options.first().map(|(_, k)| k.clone());
                    }
                    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                        close = true;
                    }
                    egui::ScrollArea::vertical().max_height(280.0).show(ui, |ui| {
                        for (i, (label, kind)) in options.iter().enumerate() {
                            let text =
                                egui::RichText::new(label).color(look::accent(kind).gamma_multiply(1.6));
                            let r = ui.selectable_label(i == 0, text);
                            if r.clicked() {
                                chosen = Some(kind.clone());
                            }
                        }
                        if options.is_empty() {
                            ui.label(egui::RichText::new("nothing matches").color(look::DIM));
                        }
                    });
                });
            });
        if let Some(kind) = chosen {
            let before = self.project.clone();
            let at = [s.world.x, s.world.y - 12.0];
            if let Some(id) = self.add_node(ctx, kind, at) {
                if let Some(p) = s.pending {
                    self.autowire(scope, p, id);
                }
                // One undo step for add + wire.
                self.undo_merge(before);
            }
            return;
        }
        if !close {
            self.ix.search = Some(s);
        }
    }

    /// Wires a dangling end to the first port of the right type on `id`.
    fn autowire(&mut self, scope: Scope, p: PortRef, id: NodeId) {
        let Some(kind) = self
            .project
            .graph(scope)
            .and_then(|g| g.nodes.get(&id))
            .map(|n| n.kind.clone())
        else {
            return;
        };
        let (ins, outs) = self.project.ports(&kind);
        let wire = if p.out {
            ins.iter().find(|x| x.ty == p.ty).map(|x| Wire {
                from: p.node,
                from_port: p.key,
                to: id,
                to_port: x.key,
            })
        } else {
            outs.iter().find(|x| x.ty == p.ty).map(|x| Wire {
                from: id,
                from_port: x.key,
                to: p.node,
                to_port: p.key,
            })
        };
        if let Some(w) = wire {
            let _ = self.project.connect(scope, w);
        }
    }

    fn draw(
        &self,
        painter: &egui::Painter,
        area: Rect,
        view: &View,
        layouts: &[(NodeId, Layout)],
        hover: Option<Pos2>,
    ) {
        let scope = self.scope();
        let z = view.zoom;
        painter.rect_filled(area, CornerRadius::ZERO, look::BG);

        // Dot grid.
        let step = 24.0 * z;
        if step > 6.0 {
            let o = view.to_screen(Pos2::ZERO);
            let x0 = area.min.x + (o.x - area.min.x).rem_euclid(step);
            let y0 = area.min.y + (o.y - area.min.y).rem_euclid(step);
            let mut y = y0;
            while y < area.max.y {
                let mut x = x0;
                while x < area.max.x {
                    painter.circle_filled(Pos2::new(x, y), 1.0, look::GRID);
                    x += step;
                }
                y += step;
            }
        }

        let Some(g) = self.project.graph(scope) else {
            return;
        };
        let path = self.live_path();
        let inst: Option<&Inst> = path.as_ref().and_then(|p| self.sim.root.at(p));
        let out_value = |node: NodeId, key: u32, ty: Ty| inst.map(|i| i.out(node, key, ty));
        let find = |id: NodeId| layouts.iter().find(|(n, _)| *n == id).map(|(_, l)| l);

        // Wires under nodes.
        for w in &g.wires {
            let (Some(a), Some(b)) = (find(w.from), find(w.to)) else {
                continue;
            };
            let Some((p, pa)) = a.outs.iter().find(|(p, _)| p.key == w.from_port) else {
                continue;
            };
            let Some((_, pb)) = b.ins.iter().find(|(p, _)| p.key == w.to_port) else {
                continue;
            };
            let live = out_value(w.from, w.from_port, p.ty).map(Value::as_bool);
            let width = if live == Some(true) { 3.0 } else { 2.0 };
            bezier(
                painter,
                view.to_screen(*pa),
                view.to_screen(*pb),
                z,
                look::wire_color(p.ty, live),
                width,
            );
        }

        let text = z > 0.4;
        let font = |size: f32| FontId::proportional(size * z);
        for (id, l) in layouts {
            let Some(node) = g.nodes.get(id) else { continue };
            let r = view.rect(l.rect);
            if !r.intersects(area) {
                continue;
            }
            let accent = look::accent(&node.kind);
            let radius = CornerRadius::same((6.0 * z).clamp(0.0, 255.0) as u8);
            let is_note = matches!(node.kind, Kind::Note { .. });
            let body = if is_note {
                Color32::from_rgb(70, 66, 40)
            } else {
                look::BODY
            };
            // Shadow, body, header.
            painter.rect_filled(
                r.translate(Vec2::splat(3.0 * z)),
                radius,
                Color32::from_black_alpha(90),
            );
            painter.rect_filled(r, radius, body);
            let h = view.rect(l.header);
            painter.rect_filled(
                h,
                CornerRadius {
                    nw: radius.nw,
                    ne: radius.ne,
                    sw: 0,
                    se: 0,
                },
                accent,
            );
            if let Kind::Block(b) = node.kind {
                // Nested look: a second outline just inside the first.
                painter.rect_stroke(
                    r.shrink(3.0 * z),
                    radius,
                    Stroke::new(1.0 * z, accent),
                    StrokeKind::Inside,
                );
                if text {
                    let n = self.project.blocks.get(&b).map_or(0, |d| d.graph.nodes.len());
                    painter.text(
                        h.right_center() - Vec2::new(6.0 * z, 0.0),
                        Align2::RIGHT_CENTER,
                        format!("{n} inside"),
                        font(10.0),
                        Color32::from_white_alpha(170),
                    );
                }
            }
            let selected = self.selected.contains(id);
            let outline = if selected {
                Stroke::new(2.0 * z, look::SELECT)
            } else {
                Stroke::new(1.0 * z, Color32::from_black_alpha(160))
            };
            painter.rect_stroke(r, radius, outline, StrokeKind::Outside);
            if text {
                painter.text(
                    h.left_center() + Vec2::new(8.0 * z, 0.0),
                    Align2::LEFT_CENTER,
                    node.title(&self.project),
                    FontId::proportional(13.0 * z),
                    Color32::WHITE,
                );
            }

            // Ports.
            for (out, ports) in [(false, &l.ins), (true, &l.outs)] {
                for (p, at) in ports {
                    let s = view.to_screen(*at);
                    let v = if out {
                        out_value(*id, p.key, p.ty)
                    } else {
                        g.wire_into(*id, p.key)
                            .and_then(|w| out_value(w.from, w.from_port, p.ty))
                    };
                    let fill = match (p.ty, v) {
                        (Ty::Bool, Some(Value::Bool(true))) => look::ON,
                        (Ty::Bool, _) => look::OFF,
                        (Ty::Num, _) => look::NUM,
                    };
                    let hot = hover.is_some_and(|h| h.distance(s) <= PORT_HIT);
                    painter.circle_filled(s, look::PORT_R * z * if hot { 1.35 } else { 1.0 }, fill);
                    painter.circle_stroke(s, look::PORT_R * z, Stroke::new(1.5 * z, look::BG));
                    if text {
                        let mut label = p.name.clone();
                        // A display shows its value big already.
                        let readout = !out && node.kind == Kind::Display;
                        if let (Ty::Num, Some(v), false) = (p.ty, v, readout) {
                            label = format!("{label} {}", look::fmt_num(v.as_num()))
                                .trim()
                                .to_string();
                        }
                        let (pos, align) = if out {
                            (s - Vec2::new(11.0 * z, 0.0), Align2::RIGHT_CENTER)
                        } else {
                            (s + Vec2::new(11.0 * z, 0.0), Align2::LEFT_CENTER)
                        };
                        painter.text(pos, align, label, font(12.0), look::TEXT);
                    }
                }
            }

            if let Some(wr) = l.widget {
                self.draw_widget(painter, view.rect(wr), z, node, *id, g, inst, text);
            }
        }

        // Dangling wire and box selection on top.
        if let Some(h) = hover {
            match &self.ix.drag {
                Drag::Wire { anchor, at, .. } => {
                    let a = view.to_screen(*at);
                    let (a, b) = if anchor.out { (a, h) } else { (h, a) };
                    bezier(
                        painter,
                        a,
                        b,
                        z,
                        look::wire_color(anchor.ty, None).gamma_multiply(1.3),
                        2.5,
                    );
                }
                Drag::Boxing { start, .. } => {
                    let r = Rect::from_two_pos(view.to_screen(*start), h);
                    painter.rect_filled(r, CornerRadius::ZERO, look::SELECT.gamma_multiply(0.08));
                    painter.rect_stroke(
                        r,
                        CornerRadius::ZERO,
                        Stroke::new(1.0, look::SELECT),
                        StrokeKind::Inside,
                    );
                }
                _ => {}
            }
        }

        if path.is_none() && scope != Scope::Root {
            painter.text(
                area.left_top() + Vec2::new(12.0, 10.0),
                Align2::LEFT_TOP,
                "editing the definition: open it through an instance to see live values",
                FontId::proportional(13.0),
                look::DIM,
            );
        }
        if g.nodes.is_empty() {
            painter.text(
                area.center(),
                Align2::CENTER_CENTER,
                if scope == Scope::Root {
                    "empty canvas: double-click to add a node"
                } else {
                    "empty block: add Pin in / Pin out nodes for its interface"
                },
                FontId::proportional(16.0),
                look::DIM,
            );
        }
    }

    #[expect(clippy::too_many_arguments)]
    fn draw_widget(
        &self,
        painter: &egui::Painter,
        r: Rect,
        z: f32,
        node: &helmsman_graph::Node,
        id: NodeId,
        g: &helmsman_graph::Graph,
        inst: Option<&Inst>,
        text: bool,
    ) {
        let input = |key: u32, ty: Ty| {
            g.wire_into(id, key)
                .and_then(|w| inst.map(|i| i.out(w.from, w.from_port, ty)))
                .unwrap_or(Value::zero(ty))
        };
        let own = |key: u32, ty: Ty| inst.map_or(Value::zero(ty), |i| i.out(id, key, ty));
        let radius = CornerRadius::same((4.0 * z) as u8);
        match &node.kind {
            Kind::Toggle { on } => {
                let track = Rect::from_center_size(r.center(), Vec2::new(46.0 * z, 22.0 * z));
                painter.rect_filled(
                    track,
                    CornerRadius::same((11.0 * z) as u8),
                    if *on {
                        look::ON.gamma_multiply(0.7)
                    } else {
                        look::OFF
                    },
                );
                let knob = if *on {
                    track.right_center() - Vec2::new(11.0 * z, 0.0)
                } else {
                    track.left_center() + Vec2::new(11.0 * z, 0.0)
                };
                painter.circle_filled(knob, 8.0 * z, Color32::WHITE);
            }
            Kind::Button => {
                let down = own(0, Ty::Bool).as_bool();
                let b = Rect::from_center_size(r.center(), Vec2::new(r.width() * 0.8, 24.0 * z));
                painter.rect_filled(
                    b,
                    radius,
                    if down {
                        look::ON.gamma_multiply(0.8)
                    } else {
                        Color32::from_rgb(70, 76, 90)
                    },
                );
                if text {
                    painter.text(
                        b.center(),
                        Align2::CENTER_CENTER,
                        if down { "pressed" } else { "hold" },
                        FontId::proportional(12.0 * z),
                        look::TEXT,
                    );
                }
            }
            Kind::Lamp => {
                let on = input(0, Ty::Bool).as_bool();
                let c = r.center();
                if on {
                    painter.circle_filled(c, 15.0 * z, look::ON.gamma_multiply(0.25));
                }
                painter.circle_filled(c, 11.0 * z, if on { look::ON } else { look::OFF });
            }
            Kind::Display | Kind::Const { .. } => {
                let v = match &node.kind {
                    Kind::Const { value } => *value,
                    _ => input(0, Ty::Num).as_num(),
                };
                painter.rect_filled(r, radius, Color32::from_rgb(16, 20, 28));
                if text {
                    painter.text(
                        r.center(),
                        Align2::CENTER_CENTER,
                        look::fmt_num(v),
                        FontId::monospace(16.0 * z),
                        look::NUM,
                    );
                }
            }
            Kind::Clock { period_ms } => {
                let q = own(0, Ty::Bool).as_bool();
                if text {
                    painter.text(
                        r.left_center(),
                        Align2::LEFT_CENTER,
                        format!("{} ms", look::fmt_num(*period_ms)),
                        FontId::proportional(12.0 * z),
                        look::DIM,
                    );
                }
                painter.circle_filled(
                    r.right_center() - Vec2::new(10.0 * z, 0.0),
                    6.0 * z,
                    if q { look::ON } else { look::OFF },
                );
            }
            Kind::Ton { preset_ms }
            | Kind::Tof { preset_ms }
            | Kind::Tp { preset_ms }
            | Kind::Tonr { preset_ms } => {
                let et = own(1, Ty::Num).as_num();
                bar(
                    painter,
                    r,
                    z,
                    if *preset_ms > 0.0 { et / preset_ms } else { 0.0 },
                    &format!("PT {} ms", look::fmt_num(*preset_ms)),
                    text,
                );
            }
            Kind::Ctu { preset } => {
                let cv = own(1, Ty::Num).as_num();
                bar(
                    painter,
                    r,
                    z,
                    if *preset > 0.0 { cv / preset } else { 0.0 },
                    &format!("PV {}", look::fmt_num(*preset)),
                    text,
                );
            }
            Kind::Block(b) => self.draw_preview(painter, r, z, *b),
            Kind::Note { text: body } if text => {
                painter.text(
                    r.left_top(),
                    Align2::LEFT_TOP,
                    body,
                    FontId::proportional(13.0 * z),
                    Color32::from_rgb(240, 230, 180),
                );
            }
            _ => {}
        }
    }

    /// A miniature of the block's contents inside the instance.
    fn draw_preview(&self, painter: &egui::Painter, r: Rect, z: f32, b: helmsman_graph::BlockId) {
        painter.rect_filled(
            r,
            CornerRadius::same((3.0 * z) as u8),
            Color32::from_rgb(28, 30, 36),
        );
        let Some(def) = self.project.blocks.get(&b) else {
            return;
        };
        let inner: Vec<(NodeId, Layout)> = def
            .graph
            .nodes
            .iter()
            .map(|(id, n)| (*id, layout(&self.project, n)))
            .collect();
        let Some(bb) = inner
            .iter()
            .map(|(_, l)| l.rect)
            .reduce(eframe::egui::Rect::union)
        else {
            return;
        };
        let r = r.shrink(4.0 * z);
        let s = (r.width() / bb.width()).min(r.height() / bb.height());
        let off = r.center() - bb.center().to_vec2() * s;
        let map = |p: Pos2| off + p.to_vec2() * s;
        for w in &def.graph.wires {
            let a = inner
                .iter()
                .find(|(id, _)| *id == w.from)
                .and_then(|(_, l)| l.outs.iter().find(|(p, _)| p.key == w.from_port));
            let c = inner
                .iter()
                .find(|(id, _)| *id == w.to)
                .and_then(|(_, l)| l.ins.iter().find(|(p, _)| p.key == w.to_port));
            if let (Some((_, a)), Some((_, c))) = (a, c) {
                painter.line_segment(
                    [map(*a), map(*c)],
                    Stroke::new(1.0, Color32::from_white_alpha(50)),
                );
            }
        }
        for (id, l) in &inner {
            let kind = &def.graph.nodes[id].kind;
            let nr = Rect::from_min_max(map(l.rect.min), map(l.rect.max));
            painter.rect_filled(nr, CornerRadius::same(1), look::accent(kind).gamma_multiply(0.85));
        }
    }
}

fn bar(painter: &egui::Painter, r: Rect, z: f32, frac: f64, label: &str, text: bool) {
    let frac = frac.clamp(0.0, 1.0) as f32;
    let track = Rect::from_min_max(
        Pos2::new(r.min.x, r.center().y - 4.0 * z),
        Pos2::new(r.max.x, r.center().y + 4.0 * z),
    );
    painter.rect_filled(track, CornerRadius::same((2.0 * z) as u8), look::OFF);
    let mut fill = track;
    fill.set_width(track.width() * frac);
    painter.rect_filled(
        fill,
        CornerRadius::same((2.0 * z) as u8),
        if frac >= 1.0 {
            look::ON
        } else {
            Color32::from_rgb(230, 150, 60)
        },
    );
    if text {
        painter.text(
            track.right_top() - Vec2::new(0.0, 2.0 * z),
            Align2::RIGHT_BOTTOM,
            label,
            FontId::proportional(10.0 * z),
            look::DIM,
        );
    }
}
