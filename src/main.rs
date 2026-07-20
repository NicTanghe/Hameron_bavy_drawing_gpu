use std::num::NonZeroU32;

use bevy::{
    asset::RenderAssetUsages,
    image::ImageSampler,
    input::{
        InputSystems,
        mouse::MouseButton,
        pen::{PenAction, PenButton, PenData, PenInput, PenPressure, PenToolKind},
    },
    prelude::*,
    render::{
        pipelined_rendering::PipelinedRenderingPlugin,
        render_resource::{Extent3d, TextureDimension, TextureFormat},
    },
    window::{CursorLeft, CursorMoved, CursorOptions, PresentMode, PrimaryWindow, WindowPlugin},
    winit::WinitSettings,
};
use hamerons_stroke_render::{
    BrushProfile, BrushSizeSpace, CanvasTileCache, CheckpointRequest, DocumentCheckpointManager,
    EffectRegistry, HameronsStrokeRenderPlugin, PaintModelRegistry, RgbaMaterial, RgbaPaintModel,
    StrokeDocument, StrokeId, StrokeInputBlocker, StrokeInputSystems, StrokePoint,
    StrokeRendererSettings,
};

const START_WIDTH: u32 = 1_200;
const START_HEIGHT: u32 = 750;
const MIN_BRUSH_SIZE: f32 = 2.0;
const MAX_BRUSH_SIZE: f32 = 180.0;
const DOCUMENT_PATH: &str = "stroke_lab.kra";
const PICKER_WIDTH: u32 = 270;
const PICKER_HEIGHT: u32 = 310;
const PICKER_TOP: f32 = 14.0;
const PICKER_RIGHT: f32 = 14.0;
const PICKER_CENTER: Vec2 = Vec2::new(135.0, 135.0);
const PICKER_RING_INNER: f32 = 91.0;
const PICKER_RING_OUTER: f32 = 111.0;
const PICKER_BLACK: Vec2 = Vec2::new(135.0, 62.0);
const PICKER_WHITE: Vec2 = Vec2::new(74.0, 185.0);
const PICKER_HUE: Vec2 = Vec2::new(196.0, 185.0);

fn main() {
    let default_plugins = DefaultPlugins
        .build()
        .set(WindowPlugin {
            primary_window: Some(Window {
                title: "Hamerons Stroke Lab".into(),
                resolution: (START_WIDTH, START_HEIGHT).into(),
                present_mode: PresentMode::AutoNoVsync,
                desired_maximum_frame_latency: NonZeroU32::new(1),
                ..default()
            }),
            ..default()
        })
        .disable::<PipelinedRenderingPlugin>();

    let mut renderer_settings = StrokeRendererSettings::default();
    renderer_settings.pen.diameter = 18.0;
    renderer_settings.eraser.diameter = 34.0;
    renderer_settings.log_diagnostics = false;

    App::new()
        .insert_resource(ClearColor(Color::srgb_u8(248, 247, 244)))
        .insert_resource(WinitSettings::continuous())
        .insert_resource(renderer_settings)
        .add_plugins(default_plugins)
        .add_plugins(HameronsStrokeRenderPlugin)
        .init_resource::<PointerState>()
        .init_resource::<MouseStroke>()
        .init_resource::<DocumentStatus>()
        .init_resource::<ColorSelector>()
        .add_systems(Startup, setup)
        .add_systems(
            PreUpdate,
            update_stroke_input_blocker
                .after(InputSystems)
                .before(StrokeInputSystems::Collect),
        )
        .add_systems(
            Update,
            (
                observe_pen_pointer,
                handle_color_selector,
                collect_mouse_strokes,
                keyboard_shortcuts,
                poll_document_checkpoint,
                draw_brush_preview,
                update_hud,
            )
                .chain(),
        )
        .run();
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Tool {
    #[default]
    Pen,
    Eraser,
}

impl Tool {
    fn label(self) -> &'static str {
        match self {
            Self::Pen => "PEN",
            Self::Eraser => "ERASER",
        }
    }

    fn profile(self, settings: &StrokeRendererSettings) -> BrushProfile {
        match self {
            Self::Pen => settings.pen,
            Self::Eraser => settings.eraser,
        }
    }

    fn profile_mut(self, settings: &mut StrokeRendererSettings) -> &mut BrushProfile {
        match self {
            Self::Pen => &mut settings.pen,
            Self::Eraser => &mut settings.eraser,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum PointerSource {
    #[default]
    Mouse,
    Tablet,
}

impl PointerSource {
    fn label(self) -> &'static str {
        match self {
            Self::Mouse => "mouse",
            Self::Tablet => "tablet",
        }
    }
}

#[derive(Resource, Default)]
struct PointerState {
    position: Option<Vec2>,
    pressure: Option<f32>,
    tilt: Vec2,
    tool: Tool,
    source: PointerSource,
    down: bool,
    pen_contact: bool,
}

impl PointerState {
    fn show_mouse(&mut self, position: Vec2, tool: Tool, down: bool) {
        self.position = Some(position);
        self.pressure = None;
        self.tilt = Vec2::ZERO;
        self.tool = tool;
        self.source = PointerSource::Mouse;
        self.down = down;
    }

    fn show_pen(&mut self, position: Vec2, tool: Tool, data: Option<&PenData>) {
        self.position = Some(position);
        self.pressure = data.and_then(|data| match data.pressure {
            Some(PenPressure::Normalized(value)) => Some(value as f32),
            Some(PenPressure::Calibrated { .. }) | None => None,
        });
        self.tilt = data.map_or(Vec2::ZERO, pen_tilt);
        self.tool = tool;
        self.source = PointerSource::Tablet;
        self.down = self.pen_contact;
    }
}

#[derive(Resource, Default)]
struct MouseStroke {
    stroke: Option<StrokeId>,
    tool: Tool,
    last_position: Option<Vec2>,
}

#[derive(Resource)]
struct DocumentStatus(String);

impl Default for DocumentStatus {
    fn default() -> Self {
        Self("document not saved".into())
    }
}

#[derive(Component)]
struct HudText;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SelectorTarget {
    Hue,
    SaturationValue,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SelectorSource {
    Mouse,
    Pen,
}

#[derive(Clone, Copy, Debug)]
struct SelectorDrag {
    target: SelectorTarget,
    source: SelectorSource,
}

#[derive(Resource)]
struct ColorSelector {
    hue: f32,
    saturation: f32,
    value: f32,
    image: Handle<Image>,
    drag: Option<SelectorDrag>,
    hovered: bool,
}

impl Default for ColorSelector {
    fn default() -> Self {
        Self {
            hue: 0.62,
            saturation: 0.55,
            value: 0.17,
            image: default(),
            drag: None,
            hovered: false,
        }
    }
}

fn setup(
    mut commands: Commands,
    mut cursor_options: Single<&mut CursorOptions, With<PrimaryWindow>>,
    mut images: ResMut<Assets<Image>>,
    mut selector: ResMut<ColorSelector>,
) {
    cursor_options.visible = false;
    commands.spawn(Camera2d);

    let image = images.add(color_selector_image(&selector));
    selector.image = image.clone();
    commands.spawn((
        ImageNode::new(image),
        Node {
            position_type: PositionType::Absolute,
            top: px(PICKER_TOP),
            right: px(PICKER_RIGHT),
            width: px(PICKER_WIDTH),
            height: px(PICKER_HEIGHT),
            border: UiRect::all(px(1)),
            ..default()
        },
        BorderColor::all(Color::srgba(0.7, 0.72, 0.76, 0.35)),
    ));
    commands.spawn((
        Text::new("ADVANCED COLOR SELECTOR"),
        TextFont::from_font_size(13.0),
        TextColor(Color::srgb(0.82, 0.83, 0.85)),
        Node {
            position_type: PositionType::Absolute,
            top: px(PICKER_TOP + 6.0),
            right: px(PICKER_RIGHT + 54.0),
            ..default()
        },
    ));

    commands.spawn((
        HudText,
        Text::new("HAMERONS STROKE  /  READY"),
        TextFont::from_font_size(14.0),
        TextColor(Color::srgb(0.94, 0.95, 0.97)),
        Node {
            position_type: PositionType::Absolute,
            top: px(16),
            left: px(16),
            padding: UiRect::axes(px(13), px(9)),
            border: UiRect::all(px(1)),
            border_radius: BorderRadius::all(px(8)),
            ..default()
        },
        BorderColor::all(Color::srgba(0.35, 0.55, 0.85, 0.35)),
        BackgroundColor(Color::srgba(0.055, 0.065, 0.085, 0.92)),
    ));
}

fn update_stroke_input_blocker(
    window: Single<&Window, With<PrimaryWindow>>,
    mut blocker: ResMut<StrokeInputBlocker>,
) {
    blocker.set_regions([picker_rect(&window)]);
}

#[allow(clippy::too_many_arguments)]
fn handle_color_selector(
    window: Single<&Window, With<PrimaryWindow>>,
    mouse_buttons: Res<ButtonInput<MouseButton>>,
    mut pen_events: MessageReader<PenInput>,
    mut selector: ResMut<ColorSelector>,
    mut images: ResMut<Assets<Image>>,
    mut rgba: ResMut<RgbaPaintModel>,
    mut settings: ResMut<StrokeRendererSettings>,
) {
    let cursor = window.cursor_position();
    selector.hovered = cursor.is_some_and(|position| picker_rect(&window).contains(position));
    let mut changed = false;
    let mut commit = false;

    if mouse_buttons.just_pressed(MouseButton::Left)
        && let Some(local) = cursor.map(|position| picker_local(&window, position))
        && let Some(target) = selector_target(local)
    {
        selector.drag = Some(SelectorDrag {
            target,
            source: SelectorSource::Mouse,
        });
        changed |= apply_selector_position(&mut selector, target, local);
    }
    if let Some(drag) = selector.drag
        && drag.source == SelectorSource::Mouse
        && mouse_buttons.pressed(MouseButton::Left)
        && let Some(local) = cursor.map(|position| picker_local(&window, position))
    {
        changed |= apply_selector_position(&mut selector, drag.target, local);
    }
    if mouse_buttons.just_released(MouseButton::Left)
        && selector
            .drag
            .is_some_and(|drag| drag.source == SelectorSource::Mouse)
    {
        selector.drag = None;
        commit = true;
    }

    for event in pen_events.read() {
        if !event.pen.primary {
            continue;
        }
        let Some(position) = event.pen.position else {
            continue;
        };
        let local = picker_local(&window, position);
        match &event.action {
            PenAction::Button {
                button: PenButton::Contact,
                state,
                ..
            } if state.is_pressed() => {
                if let Some(target) = selector_target(local) {
                    selector.drag = Some(SelectorDrag {
                        target,
                        source: SelectorSource::Pen,
                    });
                    changed |= apply_selector_position(&mut selector, target, local);
                }
            }
            PenAction::Moved(_)
                if selector
                    .drag
                    .is_some_and(|drag| drag.source == SelectorSource::Pen) =>
            {
                let target = selector.drag.expect("pen drag checked above").target;
                changed |= apply_selector_position(&mut selector, target, local);
            }
            PenAction::Button {
                button: PenButton::Contact,
                state,
                ..
            } if !state.is_pressed()
                && selector
                    .drag
                    .is_some_and(|drag| drag.source == SelectorSource::Pen) =>
            {
                let target = selector.drag.expect("pen drag checked above").target;
                changed |= apply_selector_position(&mut selector, target, local);
                selector.drag = None;
                commit = true;
            }
            PenAction::Left
                if selector
                    .drag
                    .is_some_and(|drag| drag.source == SelectorSource::Pen) =>
            {
                selector.drag = None;
                commit = true;
            }
            _ => {}
        }
    }

    if changed && let Some(mut image) = images.get_mut(&selector.image) {
        *image = color_selector_image(&selector);
    }
    if commit {
        let srgb = selector_srgb(&selector);
        let linear = srgb.map(srgb_to_linear);
        let material = rgba.add_material(RgbaMaterial::from_linear_rgba([
            linear[0], linear[1], linear[2], 1.0,
        ]));
        settings.pen.paint.material = material;
    }
}

fn picker_rect(window: &Window) -> Rect {
    let min = Vec2::new(
        window.width() - PICKER_RIGHT - PICKER_WIDTH as f32,
        PICKER_TOP,
    );
    Rect::from_corners(
        min,
        min + Vec2::new(PICKER_WIDTH as f32, PICKER_HEIGHT as f32),
    )
}

fn picker_local(window: &Window, viewport_position: Vec2) -> Vec2 {
    viewport_position - picker_rect(window).min
}

fn selector_target(local: Vec2) -> Option<SelectorTarget> {
    let ring_distance = local.distance(PICKER_CENTER);
    if (PICKER_RING_INNER..=PICKER_RING_OUTER).contains(&ring_distance) {
        Some(SelectorTarget::Hue)
    } else if triangle_barycentric(local)
        .is_some_and(|weights| weights.into_iter().all(|weight| weight >= 0.0))
    {
        Some(SelectorTarget::SaturationValue)
    } else {
        None
    }
}

fn apply_selector_position(
    selector: &mut ColorSelector,
    target: SelectorTarget,
    local: Vec2,
) -> bool {
    let before = (selector.hue, selector.saturation, selector.value);
    match target {
        SelectorTarget::Hue => {
            let delta = local - PICKER_CENTER;
            selector.hue =
                (-delta.y).atan2(delta.x).rem_euclid(std::f32::consts::TAU) / std::f32::consts::TAU;
        }
        SelectorTarget::SaturationValue => {
            if let Some(mut weights) = triangle_barycentric(local) {
                for weight in &mut weights {
                    *weight = weight.max(0.0);
                }
                let total = weights.into_iter().sum::<f32>().max(f32::EPSILON);
                let black = weights[0] / total;
                let white = weights[1] / total;
                let hue = weights[2] / total;
                selector.value = 1.0 - black;
                selector.saturation = if white + hue > f32::EPSILON {
                    hue / (white + hue)
                } else {
                    0.0
                };
            }
        }
    }
    before != (selector.hue, selector.saturation, selector.value)
}

fn triangle_barycentric(point: Vec2) -> Option<[f32; 3]> {
    let edge_0 = PICKER_WHITE - PICKER_BLACK;
    let edge_1 = PICKER_HUE - PICKER_BLACK;
    let relative = point - PICKER_BLACK;
    let denominator = edge_0.x * edge_1.y - edge_1.x * edge_0.y;
    if denominator.abs() <= f32::EPSILON {
        return None;
    }
    let white = (relative.x * edge_1.y - edge_1.x * relative.y) / denominator;
    let hue = (edge_0.x * relative.y - relative.x * edge_0.y) / denominator;
    Some([1.0 - white - hue, white, hue])
}

fn color_selector_image(selector: &ColorSelector) -> Image {
    let mut pixels = vec![0; (PICKER_WIDTH * PICKER_HEIGHT * 4) as usize];
    let hue_color = hsv_to_srgb(selector.hue, 1.0, 1.0);
    for y in 0..PICKER_HEIGHT {
        for x in 0..PICKER_WIDTH {
            let point = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
            let mut color = [0.17, 0.18, 0.20];
            let ring_distance = point.distance(PICKER_CENTER);
            if (PICKER_RING_INNER..=PICKER_RING_OUTER).contains(&ring_distance) {
                let delta = point - PICKER_CENTER;
                let hue = (-delta.y).atan2(delta.x).rem_euclid(std::f32::consts::TAU)
                    / std::f32::consts::TAU;
                color = hsv_to_srgb(hue, 1.0, 1.0);
            } else if let Some([black, white, hue]) = triangle_barycentric(point)
                && black >= 0.0
                && white >= 0.0
                && hue >= 0.0
            {
                color = [
                    white + hue_color[0] * hue,
                    white + hue_color[1] * hue,
                    white + hue_color[2] * hue,
                ];
            } else if (258..=294).contains(&y) && (18..=251).contains(&x) {
                color = selector_srgb(selector);
            }

            let ring_marker = PICKER_CENTER
                + Vec2::from_angle(-selector.hue * std::f32::consts::TAU)
                    * ((PICKER_RING_INNER + PICKER_RING_OUTER) * 0.5);
            let triangle_marker = PICKER_BLACK * (1.0 - selector.value)
                + PICKER_WHITE * (selector.value * (1.0 - selector.saturation))
                + PICKER_HUE * (selector.value * selector.saturation);
            let marker_distance = point
                .distance(ring_marker)
                .min(point.distance(triangle_marker));
            if (4.0..=6.0).contains(&marker_distance) {
                color = [0.95, 0.96, 0.98];
            } else if marker_distance < 4.0 {
                color = [0.05, 0.055, 0.065];
            }

            let index = ((y * PICKER_WIDTH + x) * 4) as usize;
            pixels[index..index + 4].copy_from_slice(&[
                color_byte(color[0]),
                color_byte(color[1]),
                color_byte(color[2]),
                255,
            ]);
        }
    }
    let mut image = Image::new(
        Extent3d {
            width: PICKER_WIDTH,
            height: PICKER_HEIGHT,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        pixels,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD,
    );
    image.sampler = ImageSampler::linear();
    image
}

fn selector_srgb(selector: &ColorSelector) -> [f32; 3] {
    hsv_to_srgb(selector.hue, selector.saturation, selector.value)
}

fn hsv_to_srgb(hue: f32, saturation: f32, value: f32) -> [f32; 3] {
    let sector = hue.rem_euclid(1.0) * 6.0;
    let chroma = value * saturation;
    let intermediate = chroma * (1.0 - (sector.rem_euclid(2.0) - 1.0).abs());
    let [red, green, blue] = match sector as u32 {
        0 => [chroma, intermediate, 0.0],
        1 => [intermediate, chroma, 0.0],
        2 => [0.0, chroma, intermediate],
        3 => [0.0, intermediate, chroma],
        4 => [intermediate, 0.0, chroma],
        _ => [chroma, 0.0, intermediate],
    };
    let match_value = value - chroma;
    [red + match_value, green + match_value, blue + match_value]
}

fn srgb_to_linear(srgb: f32) -> f32 {
    if srgb <= 0.040_45 {
        srgb / 12.92
    } else {
        ((srgb + 0.055) / 1.055).powf(2.4)
    }
}

fn color_byte(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

#[allow(clippy::too_many_arguments)]
fn collect_mouse_strokes(
    mut cursor_events: MessageReader<CursorMoved>,
    mut cursor_left_events: MessageReader<CursorLeft>,
    mouse_buttons: Res<ButtonInput<MouseButton>>,
    window: Single<&Window, With<PrimaryWindow>>,
    camera: Single<(&Camera, &GlobalTransform), With<Camera2d>>,
    settings: Res<StrokeRendererSettings>,
    mut document: ResMut<StrokeDocument>,
    mut mouse: ResMut<MouseStroke>,
    mut pointer: ResMut<PointerState>,
    selector: Res<ColorSelector>,
) {
    let mut latest_position = pointer
        .position
        .filter(|_| pointer.source == PointerSource::Mouse);
    for event in cursor_events.read() {
        latest_position = Some(event.position);
    }

    let down =
        mouse_buttons.pressed(MouseButton::Left) || mouse_buttons.pressed(MouseButton::Right);
    let tool = if mouse_buttons.pressed(MouseButton::Right) {
        Tool::Eraser
    } else if down {
        Tool::Pen
    } else {
        pointer.tool
    };
    let pressed = mouse_buttons.just_pressed(MouseButton::Left)
        || mouse_buttons.just_pressed(MouseButton::Right);

    if selector.hovered || selector.drag.is_some() {
        end_mouse_stroke(&mut document, &mut mouse);
        return;
    }

    if down && (pressed || mouse.tool != tool) {
        end_mouse_stroke(&mut document, &mut mouse);
    }

    if let Some(position) = latest_position {
        pointer.show_mouse(position, tool, down);

        if down {
            let (camera, camera_transform) = *camera;
            if let Some(point) = mouse_point(
                position,
                tool.profile(&settings),
                camera,
                camera_transform,
                &window,
            ) {
                if let Some(stroke) = mouse.stroke {
                    if mouse.last_position != Some(position) {
                        document.append_point(stroke, point);
                    }
                } else {
                    let (stroke, _) = document.begin_stroke(point, tool.profile(&settings));
                    mouse.stroke = Some(stroke);
                    mouse.tool = tool;
                }
                mouse.last_position = Some(position);
            }
        }
    }

    if !down
        || mouse_buttons.just_released(MouseButton::Left)
        || mouse_buttons.just_released(MouseButton::Right)
    {
        end_mouse_stroke(&mut document, &mut mouse);
        if pointer.source == PointerSource::Mouse {
            pointer.down = false;
        }
    }

    for _ in cursor_left_events.read() {
        end_mouse_stroke(&mut document, &mut mouse);
        if pointer.source == PointerSource::Mouse {
            pointer.position = None;
            pointer.down = false;
        }
    }
}

fn end_mouse_stroke(document: &mut StrokeDocument, mouse: &mut MouseStroke) {
    if let Some(stroke) = mouse.stroke.take() {
        document.end_stroke(stroke);
    }
    mouse.last_position = None;
}

fn mouse_point(
    viewport_position: Vec2,
    profile: BrushProfile,
    camera: &Camera,
    camera_transform: &GlobalTransform,
    window: &Window,
) -> Option<StrokePoint> {
    let position = camera
        .viewport_to_world_2d(camera_transform, viewport_position)
        .ok()?;
    let neighbor = camera
        .viewport_to_world_2d(camera_transform, viewport_position + Vec2::X)
        .or_else(|_| camera.viewport_to_world_2d(camera_transform, viewport_position - Vec2::X))
        .ok()?;
    let footprint = profile.footprint(1.0, 0.0);
    let document_scale = match profile.size_space {
        BrushSizeSpace::Document => 1.0,
        BrushSizeSpace::Screen => position.distance(neighbor) / window.scale_factor().max(0.01),
    };

    Some(StrokePoint {
        position,
        half_width: footprint.half_size.y * document_scale,
        aspect_ratio: footprint.half_size.x / footprint.half_size.y,
        flow: footprint.flow,
        orientation: Vec2::Y,
        twist_radians: 0.0,
    })
}

fn observe_pen_pointer(mut pen_events: MessageReader<PenInput>, mut pointer: ResMut<PointerState>) {
    for event in pen_events.read() {
        if !event.pen.primary {
            continue;
        }
        let tool = if event.pen.tool == PenToolKind::Eraser {
            Tool::Eraser
        } else {
            Tool::Pen
        };

        match &event.action {
            PenAction::Entered => {
                if let Some(position) = event.pen.position {
                    pointer.show_pen(position, tool, None);
                }
            }
            PenAction::Moved(data) => {
                if let Some(position) = event.pen.position {
                    pointer.show_pen(position, tool, Some(data));
                }
            }
            PenAction::Button {
                button: PenButton::Contact,
                state,
                data,
            } => {
                pointer.pen_contact = state.is_pressed();
                if let Some(position) = event.pen.position {
                    pointer.show_pen(position, tool, Some(data));
                }
            }
            PenAction::Left => {
                pointer.pen_contact = false;
                if pointer.source == PointerSource::Tablet {
                    pointer.position = None;
                    pointer.down = false;
                }
            }
            PenAction::Button { .. } => {}
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn keyboard_shortcuts(
    keys: Res<ButtonInput<KeyCode>>,
    mut window: Single<&mut Window, With<PrimaryWindow>>,
    mut settings: ResMut<StrokeRendererSettings>,
    mut document: ResMut<StrokeDocument>,
    paint_models: Res<PaintModelRegistry>,
    effects: Res<EffectRegistry>,
    mut checkpoints: ResMut<DocumentCheckpointManager>,
    mut document_status: ResMut<DocumentStatus>,
    mut mouse: ResMut<MouseStroke>,
    pointer: Res<PointerState>,
) {
    let ctrl = keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    let shift = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);

    if !ctrl && keys.just_pressed(KeyCode::KeyC) {
        document.clear();
        mouse.stroke = None;
        mouse.last_position = None;
    }
    if ctrl && keys.just_pressed(KeyCode::KeyZ) {
        if shift {
            document.redo();
        } else {
            document.undo();
        }
    }
    if ctrl && keys.just_pressed(KeyCode::KeyY) {
        document.redo();
    }
    if ctrl && keys.just_pressed(KeyCode::KeyS) {
        document_status.0 = match checkpoints.request(&document, DOCUMENT_PATH) {
            Ok(CheckpointRequest::Started) => format!("saving {DOCUMENT_PATH} in background"),
            Ok(CheckpointRequest::Busy) => "save already in progress".into(),
            Ok(CheckpointRequest::Unchanged) => "document already saved".into(),
            Err(error) => format!("save failed: {error}"),
        };
    }
    if ctrl && keys.just_pressed(KeyCode::KeyO) {
        if checkpoints.is_busy() {
            document_status.0 = "wait for the current save before loading".into();
        } else {
            match StrokeDocument::load_kra(DOCUMENT_PATH, &paint_models, &effects) {
                Ok(loaded) => {
                    let issues = loaded.compatibility_issues.len();
                    document.replace_loaded(loaded.document);
                    mouse.stroke = None;
                    mouse.last_position = None;
                    document_status.0 = if issues == 0 {
                        format!("loaded {DOCUMENT_PATH}")
                    } else {
                        format!("loaded {DOCUMENT_PATH} with {issues} compatibility issue(s)")
                    };
                }
                Err(error) => document_status.0 = format!("load failed: {error}"),
            }
        }
    }

    if !ctrl && keys.just_pressed(KeyCode::KeyN) {
        let number = document.layers().len() + 1;
        document.add_layer(format!("Layer {number}"));
    }
    if !ctrl && keys.just_pressed(KeyCode::KeyH) {
        let active = document.active_layer();
        if let Some(visible) = document.layer(active).map(|layer| layer.visible) {
            document.set_layer_visibility(active, !visible);
        }
    }
    if !ctrl && (keys.just_pressed(KeyCode::PageUp) || keys.just_pressed(KeyCode::PageDown)) {
        let active = document.active_layer();
        if let Some(index) = document.layer_index(active) {
            let target = if keys.just_pressed(KeyCode::PageUp) {
                (index + 1).min(document.layers().len().saturating_sub(1))
            } else {
                index.saturating_sub(1)
            };
            if shift {
                document.move_layer(active, target);
            } else if let Some(layer) = document.layers().get(target) {
                let id = layer.id;
                document.set_active_layer(id);
            }
        }
    }
    let opacity_delta = if !ctrl && keys.just_pressed(KeyCode::Comma) {
        -0.1
    } else if !ctrl && keys.just_pressed(KeyCode::Period) {
        0.1
    } else {
        0.0
    };
    if opacity_delta != 0.0 {
        let active = document.active_layer();
        if let Some(opacity) = document.layer(active).map(|layer| layer.opacity) {
            document.set_layer_opacity(active, opacity + opacity_delta);
        }
    }

    let size_delta = if keys.just_pressed(KeyCode::BracketLeft) {
        -2.0
    } else if keys.just_pressed(KeyCode::BracketRight) {
        2.0
    } else {
        0.0
    };
    if size_delta != 0.0 {
        let profile = pointer.tool.profile_mut(&mut settings);
        profile.diameter = (profile.diameter + size_delta).clamp(MIN_BRUSH_SIZE, MAX_BRUSH_SIZE);
    }

    if keys.just_pressed(KeyCode::KeyV) {
        window.present_mode = match window.present_mode {
            PresentMode::AutoNoVsync | PresentMode::Immediate | PresentMode::Mailbox => {
                PresentMode::AutoVsync
            }
            _ => PresentMode::AutoNoVsync,
        };
    }
}

fn poll_document_checkpoint(
    mut checkpoints: ResMut<DocumentCheckpointManager>,
    mut status: ResMut<DocumentStatus>,
) {
    if let Some(result) = checkpoints.poll() {
        status.0 = match result {
            Ok(report) => format!(
                "saved {} ({} KiB)",
                report.path.display(),
                report.bytes_written.div_ceil(1024)
            ),
            Err(error) => format!("save failed: {error}"),
        };
    }
}

fn draw_brush_preview(
    mut gizmos: Gizmos,
    window: Single<&Window, With<PrimaryWindow>>,
    pointer: Res<PointerState>,
    settings: Res<StrokeRendererSettings>,
    selector: Res<ColorSelector>,
) {
    if pointer.down || selector.hovered || selector.drag.is_some() {
        return;
    }
    let Some(position) = pointer.position else {
        return;
    };
    let pressure = if pointer.down {
        pointer.pressure.unwrap_or(1.0)
    } else {
        1.0
    };
    let footprint = pointer
        .tool
        .profile(&settings)
        .footprint(pressure, pointer.tilt.length());
    let half_size = footprint.half_size / window.scale_factor().max(0.01);
    let world_position = Vec2::new(
        position.x - window.width() * 0.5,
        window.height() * 0.5 - position.y,
    );
    let selected = selector_srgb(&selector);
    let color = match pointer.tool {
        Tool::Pen => Color::srgb(selected[0], selected[1], selected[2]),
        Tool::Eraser => Color::srgb(0.94, 0.25, 0.34),
    };

    let direction = Vec2::new(pointer.tilt.x, -pointer.tilt.y).normalize_or(Vec2::X);
    gizmos
        .ellipse_2d(
            Isometry2d::new(world_position, Rot2::radians(direction.to_angle())),
            half_size.max(Vec2::splat(0.65)),
            color,
        )
        .resolution(48);
    if pointer.tilt.length_squared() > 0.01 {
        gizmos.line_2d(
            world_position,
            world_position + direction * half_size.x.max(7.0),
            color,
        );
    }
}

fn update_hud(
    pointer: Res<PointerState>,
    settings: Res<StrokeRendererSettings>,
    document: Res<StrokeDocument>,
    tiles: Res<CanvasTileCache>,
    window: Single<&Window, With<PrimaryWindow>>,
    document_status: Res<DocumentStatus>,
    mut text: Single<&mut Text, With<HudText>>,
) {
    let pressure = pointer.pressure.map_or_else(
        || "full".to_string(),
        |value| format!("{:.0}%", value * 100.0),
    );
    let tilt = pointer.tilt.length().clamp(0.0, 90.0);
    let profile = pointer.tool.profile(&settings);
    let cache = tiles.stats();
    let active_layer = document
        .layer(document.active_layer())
        .expect("active layer must exist");
    let layer_index = document.layer_index(active_layer.id).unwrap_or(0) + 1;
    let presentation = match window.present_mode {
        PresentMode::AutoNoVsync | PresentMode::Immediate | PresentMode::Mailbox => "low latency",
        _ => "vsync",
    };
    let content = format!(
        "HAMERONS STROKE  /  {}  •  {:.0} px  •  pressure {}  •  tilt {:.0}°  •  {}\n\
         LMB/RMB draw/erase   [ ] size   C clear   Ctrl+Z undo   V {}   Ctrl+S/O save/load\n\
         LAYER {}/{}  •  {}  •  {:.0}%  •  {}   N new   PgUp/Dn select   Shift+Pg move   H hide   , . opacity\n\
         ENGINE  •  {} strokes  •  {} points  •  {} segments  •  {} resident tiles  •  {}",
        pointer.tool.label(),
        profile.diameter,
        pressure,
        tilt,
        pointer.source.label(),
        presentation,
        layer_index,
        document.layers().len(),
        active_layer.name,
        active_layer.opacity * 100.0,
        if active_layer.visible {
            "visible"
        } else {
            "hidden"
        },
        document.strokes().len(),
        document.points().len(),
        document.segments().len(),
        cache.resident_tiles,
        document_status.0,
    );
    if text.0 != content {
        text.0 = content;
    }
}

fn pen_tilt(data: &PenData) -> Vec2 {
    if let Some(tilt) = data.tilt {
        return Vec2::new(tilt.x as f32, tilt.y as f32);
    }
    if let Some(angle) = data.angle {
        let magnitude = (std::f64::consts::FRAC_PI_2 - angle.altitude)
            .to_degrees()
            .clamp(0.0, 90.0) as f32;
        return Vec2::from_angle(angle.azimuth as f32) * magnitude;
    }
    Vec2::ZERO
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_footprint_uses_the_engine_pressure_and_tilt_curve() {
        let profile = StrokeRendererSettings::default().pen;
        let full = profile.footprint(1.0, 0.0);
        let light = profile.footprint(0.0, 0.0);
        let tilted = profile.footprint(1.0, 60.0);
        assert_eq!(full.half_size.x, profile.diameter * 0.5);
        assert!(light.half_size.x < full.half_size.x);
        assert!(tilted.half_size.x > tilted.half_size.y);
    }
}
