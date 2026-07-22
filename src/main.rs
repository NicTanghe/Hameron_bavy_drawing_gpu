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
    StrokeDocument as PaintStrokeDocument, StrokeId, StrokeInputBlocker,
    StrokeInputSystems as PaintStrokeInputSystems, StrokePoint, StrokePointResampler,
    StrokeRendererSettings,
};
use vector_stroke_render::{
    CanvasExtent, DocumentLimits, Srgba8, StrokeDocument as VectorStrokeDocument,
    StrokeInputSystems as VectorStrokeInputSystems, StrokeRenderStats, VectorCanvasView,
    VectorStrokeInputBlocker, VectorStrokePlugin, VectorStrokeSettings, VectorStrokeTarget,
    load_json_file as load_vector_json, save_json_atomic as save_vector_json,
};

const START_WIDTH: u32 = 1_200;
const START_HEIGHT: u32 = 750;
const MIN_BRUSH_SIZE: f32 = 2.0;
const MAX_BRUSH_SIZE: f32 = 180.0;
const SIZE_DRAG_SENSITIVITY: f32 = 0.35;
const VECTOR_MIN_WIDTH_FACTOR: f32 = 0.08;
const PAINT_DOCUMENT_PATH: &str = "stroke_lab.kra";
const VECTOR_DOCUMENT_PATH: &str = "stroke_lab.ink.json";
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

    let vector_settings = vector_renderer_settings();

    App::new()
        .insert_resource(ClearColor(Color::srgb_u8(248, 247, 244)))
        .insert_resource(WinitSettings::continuous())
        .insert_resource(renderer_settings)
        .insert_resource(vector_settings)
        .add_plugins(default_plugins)
        .add_plugins((HameronsStrokeRenderPlugin, VectorStrokePlugin))
        .init_state::<LabState>()
        .init_resource::<PointerState>()
        .init_resource::<MouseStroke>()
        .init_resource::<BrushSizing>()
        .init_resource::<DocumentStatus>()
        .init_resource::<ColorSelector>()
        .init_resource::<VectorSession>()
        .init_resource::<PaintSession>()
        .add_systems(Startup, setup_camera)
        .add_systems(OnEnter(LabState::Menu), setup_renderer_menu)
        .add_systems(
            Update,
            renderer_menu_interaction.run_if(in_state(LabState::Menu)),
        )
        .add_systems(OnEnter(LabState::Hamerons), enter_hamerons_mode)
        .add_systems(OnExit(LabState::Hamerons), leave_hamerons_mode)
        .add_systems(OnEnter(LabState::Vector), enter_vector_mode)
        .add_systems(OnExit(LabState::Vector), leave_vector_mode)
        .add_systems(
            PreUpdate,
            update_input_blockers
                .after(InputSystems)
                .before(PaintStrokeInputSystems::Collect)
                .before(VectorStrokeInputSystems::Collect),
        )
        .add_systems(
            Update,
            (handle_color_selector, return_to_menu)
                .chain()
                .run_if(not(in_state(LabState::Menu))),
        )
        .add_systems(
            Update,
            (
                observe_pen_pointer,
                collect_mouse_strokes,
                keyboard_shortcuts,
                poll_document_checkpoint,
                draw_brush_preview,
                update_hud,
            )
                .chain()
                .run_if(in_state(LabState::Hamerons)),
        )
        .add_systems(
            Update,
            (
                observe_vector_pointer,
                vector_keyboard_shortcuts,
                draw_vector_brush_preview,
                update_vector_hud,
            )
                .chain()
                .run_if(in_state(LabState::Vector)),
        )
        .run();
}

fn vector_renderer_settings() -> VectorStrokeSettings {
    let mut settings = VectorStrokeSettings::default();
    settings.pen_style.base_width = 18.0;
    settings.pen_style.min_width_factor = VECTOR_MIN_WIDTH_FACTOR;
    settings.eraser_radius = 17.0;
    settings
}

#[derive(States, Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
enum LabState {
    #[default]
    Menu,
    Hamerons,
    Vector,
}

#[derive(Component, Clone, Copy)]
struct RendererMenuButton(LabState);

type RendererMenuInteraction<'w, 's> = Query<
    'w,
    's,
    (
        &'static Interaction,
        &'static RendererMenuButton,
        &'static mut BackgroundColor,
    ),
    (Changed<Interaction>, With<Button>),
>;

#[derive(Resource, Default)]
struct VectorSession {
    canvas: Option<vector_stroke_render::CanvasId>,
    layer: Option<vector_stroke_render::LayerId>,
    status: String,
}

#[derive(Resource, Default)]
struct PaintSession {
    document: Option<PaintStrokeDocument>,
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
    resampler: Option<StrokePointResampler>,
}

#[derive(Clone, Copy)]
struct SizeGesture {
    source: PointerSource,
    tool: Tool,
    origin: Vec2,
    starting_size: f32,
}

#[derive(Resource, Default)]
struct BrushSizing {
    gesture: Option<SizeGesture>,
}

impl BrushSizing {
    fn end(&mut self, source: PointerSource) {
        if self.gesture.is_some_and(|gesture| gesture.source == source) {
            self.gesture = None;
        }
    }

    fn active_for(&self, source: PointerSource) -> bool {
        self.gesture.is_some_and(|gesture| gesture.source == source)
    }
}

fn update_size_gesture(
    source: PointerSource,
    tool: Tool,
    position: Vec2,
    sizing: &mut BrushSizing,
    settings: &mut StrokeRendererSettings,
) {
    let start_new = sizing
        .gesture
        .is_none_or(|gesture| gesture.source != source || gesture.tool != tool);
    if start_new {
        sizing.gesture = Some(SizeGesture {
            source,
            tool,
            origin: position,
            starting_size: tool.profile(settings).diameter,
        });
    }

    let gesture = sizing
        .gesture
        .expect("a size gesture must exist after initialization");
    tool.profile_mut(settings).diameter =
        dragged_brush_size(gesture.starting_size, gesture.origin, position);
}

fn dragged_brush_size(starting_size: f32, origin: Vec2, position: Vec2) -> f32 {
    let delta = (position.x - origin.x) - (position.y - origin.y);
    (starting_size + delta * SIZE_DRAG_SENSITIVITY).clamp(MIN_BRUSH_SIZE, MAX_BRUSH_SIZE)
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

fn setup_camera(mut commands: Commands) {
    commands.spawn((Camera2d, Msaa::Off));
}

fn setup_renderer_menu(
    mut commands: Commands,
    mut cursor_options: Single<&mut CursorOptions, With<PrimaryWindow>>,
    mut window: Single<&mut Window, With<PrimaryWindow>>,
) {
    cursor_options.visible = true;
    window.title = "Stroke Drawing Test — Choose a Renderer".into();

    commands
        .spawn((
            DespawnOnExit(LabState::Menu),
            Node {
                position_type: PositionType::Absolute,
                width: percent(100),
                height: percent(100),
                display: Display::Flex,
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                row_gap: px(18),
                ..default()
            },
            BackgroundColor(Color::srgb_u8(21, 24, 31)),
        ))
        .with_children(|parent| {
            parent.spawn((
                Text::new("STROKE DRAWING TEST"),
                TextFont::from_font_size(34.0),
                TextColor(Color::srgb_u8(238, 241, 247)),
            ));
            parent.spawn((
                Text::new("Choose the renderer used for this drawing session"),
                TextFont::from_font_size(16.0),
                TextColor(Color::srgb_u8(155, 164, 181)),
                Node {
                    margin: UiRect::bottom(px(14)),
                    ..default()
                },
            ));
            spawn_renderer_button(
                parent,
                LabState::Hamerons,
                "1   HAMERONS PAINT RENDERER",
                "GPU paint layers · saves stroke_lab.kra",
            );
            spawn_renderer_button(
                parent,
                LabState::Vector,
                "2   VECTOR STROKE RENDERER",
                "Editable pressure-sensitive paths · saves stroke_lab.ink.json",
            );
            parent.spawn((
                Text::new("Press 1 or 2, or select with the pointer"),
                TextFont::from_font_size(13.0),
                TextColor(Color::srgb_u8(111, 120, 138)),
                Node {
                    margin: UiRect::top(px(12)),
                    ..default()
                },
            ));
        });
}

fn spawn_renderer_button(
    parent: &mut ChildSpawnerCommands,
    state: LabState,
    title: &'static str,
    subtitle: &'static str,
) {
    parent
        .spawn((
            Button,
            RendererMenuButton(state),
            Node {
                width: px(510),
                min_height: px(88),
                display: Display::Flex,
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::FlexStart,
                justify_content: JustifyContent::Center,
                padding: UiRect::axes(px(22), px(12)),
                border: UiRect::all(px(1)),
                border_radius: BorderRadius::all(px(10)),
                ..default()
            },
            BorderColor::all(Color::srgba(0.38, 0.50, 0.72, 0.55)),
            BackgroundColor(Color::srgb_u8(34, 40, 52)),
        ))
        .with_children(|button| {
            button.spawn((
                Text::new(title),
                TextFont::from_font_size(19.0),
                TextColor(Color::srgb_u8(235, 239, 247)),
            ));
            button.spawn((
                Text::new(subtitle),
                TextFont::from_font_size(13.0),
                TextColor(Color::srgb_u8(154, 168, 192)),
                Node {
                    margin: UiRect::top(px(5)),
                    ..default()
                },
            ));
        });
}

fn renderer_menu_interaction(
    keys: Res<ButtonInput<KeyCode>>,
    mut buttons: RendererMenuInteraction,
    mut next_state: ResMut<NextState<LabState>>,
) {
    if keys.just_pressed(KeyCode::Digit1) || keys.just_pressed(KeyCode::Numpad1) {
        next_state.set(LabState::Hamerons);
    } else if keys.just_pressed(KeyCode::Digit2) || keys.just_pressed(KeyCode::Numpad2) {
        next_state.set(LabState::Vector);
    }

    for (interaction, button, mut background) in &mut buttons {
        *background = match *interaction {
            Interaction::Pressed => {
                next_state.set(button.0);
                BackgroundColor(Color::srgb_u8(57, 78, 116))
            }
            Interaction::Hovered => BackgroundColor(Color::srgb_u8(47, 58, 78)),
            Interaction::None => BackgroundColor(Color::srgb_u8(34, 40, 52)),
        };
    }
}

#[allow(clippy::too_many_arguments)]
fn enter_hamerons_mode(
    mut commands: Commands,
    mut cursor_options: Single<&mut CursorOptions, With<PrimaryWindow>>,
    mut window: Single<&mut Window, With<PrimaryWindow>>,
    mut camera_msaa: Single<&mut Msaa, With<Camera2d>>,
    mut images: ResMut<Assets<Image>>,
    mut selector: ResMut<ColorSelector>,
    mut document: ResMut<PaintStrokeDocument>,
    mut session: ResMut<PaintSession>,
) {
    cursor_options.visible = false;
    window.title = "Stroke Drawing Test — Hamerons Paint Renderer".into();
    **camera_msaa = Msaa::Off;
    if let Some(stored) = session.document.take() {
        document.replace_loaded(stored);
    }
    spawn_drawing_ui(
        &mut commands,
        &mut images,
        &mut selector,
        LabState::Hamerons,
        "HAMERONS PAINT  /  READY",
    );
}

fn leave_hamerons_mode(
    mut document: ResMut<PaintStrokeDocument>,
    mut session: ResMut<PaintSession>,
    mut mouse: ResMut<MouseStroke>,
    mut pointer: ResMut<PointerState>,
    mut selector: ResMut<ColorSelector>,
) {
    end_mouse_stroke(&mut document, &mut mouse);
    let mut stored = PaintStrokeDocument::default();
    std::mem::swap(&mut *document, &mut stored);
    document.replace_loaded(PaintStrokeDocument::default());
    session.document = Some(stored);
    *pointer = default();
    selector.drag = None;
    selector.hovered = false;
}

#[allow(clippy::too_many_arguments)]
fn enter_vector_mode(
    mut commands: Commands,
    mut cursor_options: Single<&mut CursorOptions, With<PrimaryWindow>>,
    mut window: Single<&mut Window, With<PrimaryWindow>>,
    mut camera_msaa: Single<&mut Msaa, With<Camera2d>>,
    mut images: ResMut<Assets<Image>>,
    mut selector: ResMut<ColorSelector>,
    mut document: ResMut<VectorStrokeDocument>,
    mut target: ResMut<VectorStrokeTarget>,
    mut session: ResMut<VectorSession>,
) {
    cursor_options.visible = false;
    window.title = "Stroke Drawing Test — Vector Stroke Renderer".into();
    **camera_msaa = Msaa::Sample4;

    let (canvas, layer, extent) = ensure_vector_surface(&mut document, &mut session);
    target.set(canvas, layer);
    commands.spawn((
        VectorCanvasView::new(canvas),
        Transform::from_xyz(-extent.width * 0.5, extent.height * 0.5, 0.0),
        DespawnOnExit(LabState::Vector),
    ));
    spawn_drawing_ui(
        &mut commands,
        &mut images,
        &mut selector,
        LabState::Vector,
        "VECTOR STROKE  /  READY",
    );
}

fn leave_vector_mode(
    mut target: ResMut<VectorStrokeTarget>,
    mut pointer: ResMut<PointerState>,
    mut sizing: ResMut<BrushSizing>,
    mut selector: ResMut<ColorSelector>,
) {
    target.clear();
    *pointer = default();
    sizing.gesture = None;
    selector.drag = None;
    selector.hovered = false;
}

fn ensure_vector_surface(
    document: &mut VectorStrokeDocument,
    session: &mut VectorSession,
) -> (
    vector_stroke_render::CanvasId,
    vector_stroke_render::LayerId,
    CanvasExtent,
) {
    if let (Some(canvas), Some(layer)) = (session.canvas, session.layer)
        && let Ok(canvas_data) = document.canvas(canvas)
        && canvas_data
            .layers
            .iter()
            .any(|candidate| candidate.id == layer)
    {
        return (canvas, layer, canvas_data.extent);
    }

    if let Some(canvas_data) = document.canvases().first()
        && let Some(layer_data) = canvas_data.layers.first()
    {
        session.canvas = Some(canvas_data.id);
        session.layer = Some(layer_data.id);
        return (canvas_data.id, layer_data.id, canvas_data.extent);
    }

    let extent = CanvasExtent::new(START_WIDTH as f32, START_HEIGHT as f32);
    let canvas = document
        .create_canvas(extent, None)
        .expect("the default vector canvas extent is valid");
    let layer = document.canvas(canvas).expect("new canvas exists").layers[0].id;
    session.canvas = Some(canvas);
    session.layer = Some(layer);
    (canvas, layer, extent)
}

fn spawn_drawing_ui(
    commands: &mut Commands,
    images: &mut Assets<Image>,
    selector: &mut ColorSelector,
    state: LabState,
    initial_hud: &'static str,
) {
    let image = images.add(color_selector_image(selector));
    selector.image = image.clone();
    commands.spawn((
        DespawnOnExit(state),
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
        DespawnOnExit(state),
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
        DespawnOnExit(state),
        HudText,
        Text::new(initial_hud),
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

fn update_input_blockers(
    window: Single<&Window, With<PrimaryWindow>>,
    keys: Res<ButtonInput<KeyCode>>,
    state: Res<State<LabState>>,
    mut paint_blocker: ResMut<StrokeInputBlocker>,
    mut vector_blocker: ResMut<VectorStrokeInputBlocker>,
) {
    let shift = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    let full_window = Rect::from_corners(Vec2::ZERO, Vec2::new(window.width(), window.height()));
    if *state.get() != LabState::Hamerons || shift {
        paint_blocker.set_regions([full_window]);
    } else {
        paint_blocker.set_regions([picker_rect(&window)]);
    }
    if *state.get() != LabState::Vector || shift {
        vector_blocker.set_regions([full_window]);
    } else {
        vector_blocker.set_regions([picker_rect(&window)]);
    }
}

fn return_to_menu(keys: Res<ButtonInput<KeyCode>>, mut next_state: ResMut<NextState<LabState>>) {
    if keys.just_pressed(KeyCode::Escape) {
        next_state.set(LabState::Menu);
    }
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
    mut vector_settings: ResMut<VectorStrokeSettings>,
    state: Res<State<LabState>>,
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
        match state.get() {
            LabState::Hamerons => {
                let linear = srgb.map(srgb_to_linear);
                let material = rgba.add_material(RgbaMaterial::from_linear_rgba([
                    linear[0], linear[1], linear[2], 1.0,
                ]));
                settings.pen.paint.material = material;
            }
            LabState::Vector => {
                vector_settings.pen_style.color = Srgba8::new(
                    color_byte(srgb[0]),
                    color_byte(srgb[1]),
                    color_byte(srgb[2]),
                    255,
                );
            }
            LabState::Menu => {}
        }
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
    keys: Res<ButtonInput<KeyCode>>,
    mouse_buttons: Res<ButtonInput<MouseButton>>,
    window: Single<&Window, With<PrimaryWindow>>,
    camera: Single<(&Camera, &GlobalTransform), With<Camera2d>>,
    mut settings: ResMut<StrokeRendererSettings>,
    mut document: ResMut<PaintStrokeDocument>,
    mut mouse: ResMut<MouseStroke>,
    mut sizing: ResMut<BrushSizing>,
    mut pointer: ResMut<PointerState>,
    selector: Res<ColorSelector>,
) {
    let mut positions: Vec<_> = cursor_events.read().map(|event| event.position).collect();
    if positions.is_empty()
        && let Some(position) = pointer
            .position
            .filter(|_| pointer.source == PointerSource::Mouse)
    {
        positions.push(position);
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
    let shift = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);

    if selector.hovered || selector.drag.is_some() {
        end_mouse_stroke(&mut document, &mut mouse);
        sizing.end(PointerSource::Mouse);
        return;
    }

    if down && (pressed || mouse.tool != tool) {
        end_mouse_stroke(&mut document, &mut mouse);
    }

    for position in positions {
        pointer.show_mouse(position, tool, down);

        if down && shift {
            end_mouse_stroke(&mut document, &mut mouse);
            update_size_gesture(
                PointerSource::Mouse,
                tool,
                position,
                &mut sizing,
                &mut settings,
            );
        } else if down {
            sizing.end(PointerSource::Mouse);
            let (camera, camera_transform) = *camera;
            if let Some(point) = mouse_point(
                position,
                tool.profile(&settings),
                camera,
                camera_transform,
                &window,
            ) {
                if let Some(stroke) = mouse.stroke {
                    if let Some(resampler) = &mut mouse.resampler {
                        resampler.push(point, |point| {
                            document.append_point(stroke, point);
                        });
                    }
                } else {
                    let (stroke, _) = document.begin_stroke(point, tool.profile(&settings));
                    mouse.stroke = Some(stroke);
                    mouse.tool = tool;
                    mouse.resampler = Some(StrokePointResampler::new(point));
                }
            }
        }
    }

    if !down
        || mouse_buttons.just_released(MouseButton::Left)
        || mouse_buttons.just_released(MouseButton::Right)
    {
        end_mouse_stroke(&mut document, &mut mouse);
        sizing.end(PointerSource::Mouse);
        if pointer.source == PointerSource::Mouse {
            pointer.down = false;
        }
    }

    for _ in cursor_left_events.read() {
        end_mouse_stroke(&mut document, &mut mouse);
        sizing.end(PointerSource::Mouse);
        if pointer.source == PointerSource::Mouse {
            pointer.position = None;
            pointer.down = false;
        }
    }
}

fn end_mouse_stroke(document: &mut PaintStrokeDocument, mouse: &mut MouseStroke) {
    if let Some(stroke) = mouse.stroke.take() {
        if let Some(mut resampler) = mouse.resampler.take() {
            resampler.finish(|point| {
                document.append_point(stroke, point);
            });
        }
        document.end_stroke(stroke);
    }
    mouse.resampler = None;
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

fn observe_pen_pointer(
    mut pen_events: MessageReader<PenInput>,
    keys: Res<ButtonInput<KeyCode>>,
    window: Single<&Window, With<PrimaryWindow>>,
    mut pointer: ResMut<PointerState>,
    mut sizing: ResMut<BrushSizing>,
    mut settings: ResMut<StrokeRendererSettings>,
) {
    let shift = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    if !shift {
        sizing.end(PointerSource::Tablet);
    }

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
                    if pointer.pen_contact && shift && !picker_rect(&window).contains(position) {
                        update_size_gesture(
                            PointerSource::Tablet,
                            tool,
                            position,
                            &mut sizing,
                            &mut settings,
                        );
                    }
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
                    if state.is_pressed() && shift && !picker_rect(&window).contains(position) {
                        update_size_gesture(
                            PointerSource::Tablet,
                            tool,
                            position,
                            &mut sizing,
                            &mut settings,
                        );
                    }
                }
                if !state.is_pressed() {
                    sizing.end(PointerSource::Tablet);
                }
            }
            PenAction::Left => {
                pointer.pen_contact = false;
                sizing.end(PointerSource::Tablet);
                if pointer.source == PointerSource::Tablet {
                    pointer.position = None;
                    pointer.down = false;
                }
            }
            PenAction::Button { .. } => {}
        }
    }
}

fn vector_tool_size(tool: Tool, settings: &VectorStrokeSettings) -> f32 {
    match tool {
        Tool::Pen => settings.pen_style.base_width,
        Tool::Eraser => settings.eraser_radius * 2.0,
    }
}

fn set_vector_tool_size(tool: Tool, settings: &mut VectorStrokeSettings, diameter: f32) {
    let diameter = diameter.clamp(MIN_BRUSH_SIZE, MAX_BRUSH_SIZE);
    match tool {
        Tool::Pen => settings.pen_style.base_width = diameter,
        Tool::Eraser => settings.eraser_radius = diameter * 0.5,
    }
}

fn update_vector_size_gesture(
    source: PointerSource,
    tool: Tool,
    position: Vec2,
    sizing: &mut BrushSizing,
    settings: &mut VectorStrokeSettings,
) {
    let start_new = sizing
        .gesture
        .is_none_or(|gesture| gesture.source != source || gesture.tool != tool);
    if start_new {
        sizing.gesture = Some(SizeGesture {
            source,
            tool,
            origin: position,
            starting_size: vector_tool_size(tool, settings),
        });
    }
    let gesture = sizing
        .gesture
        .expect("a vector size gesture must exist after initialization");
    set_vector_tool_size(
        tool,
        settings,
        dragged_brush_size(gesture.starting_size, gesture.origin, position),
    );
}

#[allow(clippy::too_many_arguments)]
fn observe_vector_pointer(
    mut cursor_events: MessageReader<CursorMoved>,
    mut cursor_left_events: MessageReader<CursorLeft>,
    mut pen_events: MessageReader<PenInput>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse_buttons: Res<ButtonInput<MouseButton>>,
    window: Single<&Window, With<PrimaryWindow>>,
    selector: Res<ColorSelector>,
    mut pointer: ResMut<PointerState>,
    mut sizing: ResMut<BrushSizing>,
    mut settings: ResMut<VectorStrokeSettings>,
) {
    let shift = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    let mouse_down =
        mouse_buttons.pressed(MouseButton::Left) || mouse_buttons.pressed(MouseButton::Right);
    let mouse_tool = if mouse_buttons.pressed(MouseButton::Right) {
        Tool::Eraser
    } else {
        Tool::Pen
    };

    for event in cursor_events.read() {
        pointer.show_mouse(event.position, mouse_tool, mouse_down);
        if mouse_down
            && shift
            && !selector.hovered
            && selector.drag.is_none()
            && !picker_rect(&window).contains(event.position)
        {
            update_vector_size_gesture(
                PointerSource::Mouse,
                mouse_tool,
                event.position,
                &mut sizing,
                &mut settings,
            );
        }
    }
    if !mouse_down || !shift {
        sizing.end(PointerSource::Mouse);
    }
    for _ in cursor_left_events.read() {
        sizing.end(PointerSource::Mouse);
        if pointer.source == PointerSource::Mouse {
            pointer.position = None;
            pointer.down = false;
        }
    }

    if !shift {
        sizing.end(PointerSource::Tablet);
    }
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
                    if pointer.pen_contact
                        && shift
                        && !selector.hovered
                        && selector.drag.is_none()
                        && !picker_rect(&window).contains(position)
                    {
                        update_vector_size_gesture(
                            PointerSource::Tablet,
                            tool,
                            position,
                            &mut sizing,
                            &mut settings,
                        );
                    }
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
                    if state.is_pressed() && shift && !picker_rect(&window).contains(position) {
                        update_vector_size_gesture(
                            PointerSource::Tablet,
                            tool,
                            position,
                            &mut sizing,
                            &mut settings,
                        );
                    }
                }
                if !state.is_pressed() {
                    sizing.end(PointerSource::Tablet);
                }
            }
            PenAction::Left => {
                pointer.pen_contact = false;
                sizing.end(PointerSource::Tablet);
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
fn vector_keyboard_shortcuts(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    mut window: Single<&mut Window, With<PrimaryWindow>>,
    mut settings: ResMut<VectorStrokeSettings>,
    mut document: ResMut<VectorStrokeDocument>,
    mut target: ResMut<VectorStrokeTarget>,
    mut session: ResMut<VectorSession>,
    pointer: Res<PointerState>,
    mut views: Query<(Entity, &mut VectorCanvasView, &mut Transform)>,
) {
    let ctrl = keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    let shift = keys.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    let Some(canvas) = session.canvas else {
        return;
    };
    let Some(layer) = session.layer else {
        return;
    };

    if !ctrl && keys.just_pressed(KeyCode::KeyC) {
        session.status = match document.clear_canvas(canvas) {
            Ok(count) => format!("cleared {count} vector stroke(s)"),
            Err(error) => format!("clear failed: {error}"),
        };
    }
    if ctrl && keys.just_pressed(KeyCode::KeyZ) {
        let result = if shift {
            document.redo()
        } else {
            document.undo()
        };
        if let Err(error) = result {
            session.status = format!("history: {error}");
        }
    }
    if ctrl
        && keys.just_pressed(KeyCode::KeyY)
        && let Err(error) = document.redo()
    {
        session.status = format!("history: {error}");
    }
    if ctrl && keys.just_pressed(KeyCode::KeyS) {
        session.status = match save_vector_json(VECTOR_DOCUMENT_PATH, &document) {
            Ok(()) => format!("saved {VECTOR_DOCUMENT_PATH}"),
            Err(error) => format!("save failed: {error}"),
        };
    }
    if ctrl && keys.just_pressed(KeyCode::KeyO) {
        if document.has_active_strokes() {
            session.status = "finish the active stroke before loading".into();
        } else {
            match load_vector_json(VECTOR_DOCUMENT_PATH, DocumentLimits::default()) {
                Ok(mut loaded) => {
                    let (new_canvas, new_layer, extent) =
                        ensure_vector_surface(&mut loaded, &mut session);
                    *document = loaded;
                    target.set(new_canvas, new_layer);
                    if let Some((_, mut view, mut transform)) = views.iter_mut().next() {
                        *view = VectorCanvasView::new(new_canvas);
                        *transform =
                            Transform::from_xyz(-extent.width * 0.5, extent.height * 0.5, 0.0);
                    } else {
                        commands.spawn((
                            VectorCanvasView::new(new_canvas),
                            Transform::from_xyz(-extent.width * 0.5, extent.height * 0.5, 0.0),
                            DespawnOnExit(LabState::Vector),
                        ));
                    }
                    session.status = format!("loaded {VECTOR_DOCUMENT_PATH}");
                }
                Err(error) => session.status = format!("load failed: {error}"),
            }
        }
    }

    if !ctrl && keys.just_pressed(KeyCode::KeyN) {
        let number = document
            .canvas(canvas)
            .map_or(1, |canvas| canvas.layers.len() + 1);
        match document.create_layer(canvas, format!("Layer {number}")) {
            Ok(new_layer) => {
                session.layer = Some(new_layer);
                target.set(canvas, new_layer);
                session.status = format!("created Layer {number}");
            }
            Err(error) => session.status = format!("new layer failed: {error}"),
        }
    }
    if !ctrl && keys.just_pressed(KeyCode::KeyH) {
        let visible = document.layer(canvas, layer).map(|layer| layer.visible);
        if let Ok(visible) = visible
            && let Err(error) = document.set_layer_visibility(canvas, layer, !visible)
        {
            session.status = format!("visibility failed: {error}");
        }
    }
    if !ctrl && (keys.just_pressed(KeyCode::PageUp) || keys.just_pressed(KeyCode::PageDown)) {
        let layer_data = document.canvas(canvas).ok().and_then(|canvas_data| {
            let index = canvas_data
                .layers
                .iter()
                .position(|candidate| candidate.id == layer)?;
            let target_index = if keys.just_pressed(KeyCode::PageUp) {
                (index + 1).min(canvas_data.layers.len().saturating_sub(1))
            } else {
                index.saturating_sub(1)
            };
            Some((target_index, canvas_data.layers[target_index].id))
        });
        if let Some((target_index, target_layer)) = layer_data {
            if shift {
                if let Err(error) = document.reorder_layer(canvas, layer, target_index) {
                    session.status = format!("move layer failed: {error}");
                }
            } else {
                session.layer = Some(target_layer);
                target.set(canvas, target_layer);
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
    if opacity_delta != 0.0
        && let Ok(opacity) = document.layer(canvas, layer).map(|layer| layer.opacity)
        && let Err(error) = document.set_layer_opacity(canvas, layer, opacity + opacity_delta)
    {
        session.status = format!("opacity failed: {error}");
    }

    let size_delta = if keys.just_pressed(KeyCode::BracketLeft) {
        -2.0
    } else if keys.just_pressed(KeyCode::BracketRight) {
        2.0
    } else {
        0.0
    };
    if size_delta != 0.0 {
        let diameter = vector_tool_size(pointer.tool, &settings) + size_delta;
        set_vector_tool_size(pointer.tool, &mut settings, diameter);
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

fn draw_vector_brush_preview(
    mut gizmos: Gizmos,
    window: Single<&Window, With<PrimaryWindow>>,
    pointer: Res<PointerState>,
    sizing: Res<BrushSizing>,
    settings: Res<VectorStrokeSettings>,
    selector: Res<ColorSelector>,
) {
    if (pointer.down && !sizing.active_for(pointer.source))
        || selector.hovered
        || selector.drag.is_some()
    {
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
    let diameter = match pointer.tool {
        Tool::Pen => settings.pen_style.width_at(pressure),
        Tool::Eraser => settings.eraser_radius * 2.0,
    };
    let world_position = Vec2::new(
        position.x - window.width() * 0.5,
        window.height() * 0.5 - position.y,
    );
    let color = match pointer.tool {
        Tool::Pen => {
            let [red, green, blue, _] = settings.pen_style.color.as_f32();
            Color::srgb(red, green, blue)
        }
        Tool::Eraser => Color::srgb(0.94, 0.25, 0.34),
    };
    gizmos
        .circle_2d(world_position, (diameter * 0.5).max(0.65), color)
        .resolution(48);
}

fn update_vector_hud(
    pointer: Res<PointerState>,
    settings: Res<VectorStrokeSettings>,
    document: Res<VectorStrokeDocument>,
    stats: Res<StrokeRenderStats>,
    session: Res<VectorSession>,
    window: Single<&Window, With<PrimaryWindow>>,
    mut text: Single<&mut Text, With<HudText>>,
) {
    let pressure = pointer.pressure.map_or_else(
        || "full".to_string(),
        |value| format!("{:.0}%", value * 100.0),
    );
    let presentation = match window.present_mode {
        PresentMode::AutoNoVsync | PresentMode::Immediate | PresentMode::Mailbox => "low latency",
        _ => "vsync",
    };
    let diameter = vector_tool_size(pointer.tool, &settings);
    let stroke_count = document.strokes().count();
    let point_count = document
        .strokes()
        .map(|(_, _, stroke)| stroke.points.len())
        .sum::<usize>();
    let layer_summary = session
        .canvas
        .zip(session.layer)
        .and_then(|(canvas_id, layer_id)| {
            let canvas = document.canvas(canvas_id).ok()?;
            let index = canvas
                .layers
                .iter()
                .position(|layer| layer.id == layer_id)?;
            let layer = &canvas.layers[index];
            Some(format!(
                "LAYER {}/{}  •  {}  •  {:.0}%  •  {}",
                index + 1,
                canvas.layers.len(),
                layer.name,
                layer.opacity * 100.0,
                if layer.visible { "visible" } else { "hidden" }
            ))
        })
        .unwrap_or_else(|| "LAYER unavailable".into());
    let content = format!(
        "VECTOR STROKE  /  {}  •  {:.0} px  •  pressure {}  •  {}  •  {}\n\
         LMB/RMB draw/erase   Shift+drag size   [ ] size   C clear   Ctrl+Z undo   V {}   Ctrl+S/O JSON   Esc menu\n\
         {}   N new   PgUp/Dn select   Shift+Pg move   H hide   , . opacity\n\
         ENGINE  •  {} strokes  •  {} points  •  {} visible meshes  •  {} cached meshes  •  {}",
        pointer.tool.label(),
        diameter,
        pressure,
        pointer.source.label(),
        settings.pen_style.color,
        presentation,
        layer_summary,
        stroke_count,
        point_count,
        stats.visible_strokes,
        stats.cached_strokes,
        if session.status.is_empty() {
            "document not saved"
        } else {
            &session.status
        },
    );
    if text.0 != content {
        text.0 = content;
    }
}

#[allow(clippy::too_many_arguments)]
fn keyboard_shortcuts(
    keys: Res<ButtonInput<KeyCode>>,
    mut window: Single<&mut Window, With<PrimaryWindow>>,
    mut settings: ResMut<StrokeRendererSettings>,
    mut document: ResMut<PaintStrokeDocument>,
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
        mouse.resampler = None;
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
        document_status.0 = match checkpoints.request(&document, PAINT_DOCUMENT_PATH) {
            Ok(CheckpointRequest::Started) => {
                format!("saving {PAINT_DOCUMENT_PATH} in background")
            }
            Ok(CheckpointRequest::Busy) => "save already in progress".into(),
            Ok(CheckpointRequest::Unchanged) => "document already saved".into(),
            Err(error) => format!("save failed: {error}"),
        };
    }
    if ctrl && keys.just_pressed(KeyCode::KeyO) {
        if checkpoints.is_busy() {
            document_status.0 = "wait for the current save before loading".into();
        } else {
            match PaintStrokeDocument::load_kra(PAINT_DOCUMENT_PATH, &paint_models, &effects) {
                Ok(loaded) => {
                    let issues = loaded.compatibility_issues.len();
                    document.replace_loaded(loaded.document);
                    mouse.stroke = None;
                    mouse.resampler = None;
                    document_status.0 = if issues == 0 {
                        format!("loaded {PAINT_DOCUMENT_PATH}")
                    } else {
                        format!("loaded {PAINT_DOCUMENT_PATH} with {issues} compatibility issue(s)")
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
    sizing: Res<BrushSizing>,
    settings: Res<StrokeRendererSettings>,
    selector: Res<ColorSelector>,
) {
    if (pointer.down && !sizing.active_for(pointer.source))
        || selector.hovered
        || selector.drag.is_some()
    {
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
    document: Res<PaintStrokeDocument>,
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
         LMB/RMB draw/erase   Shift+drag size   [ ] size   C clear   Ctrl+Z undo   V {}   Ctrl+S/O save/load   Esc menu\n\
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
    if let Some(angle) = data.angle {
        let magnitude = (std::f64::consts::FRAC_PI_2 - angle.altitude)
            .to_degrees()
            .clamp(0.0, 90.0) as f32;
        return Vec2::from_angle(angle.azimuth as f32) * magnitude;
    }
    if let Some(tilt) = data.tilt {
        let projection = Vec2::new(
            (tilt.x as f32).to_radians().tan(),
            (tilt.y as f32).to_radians().tan(),
        );
        return projection.normalize_or_zero() * projection.length().atan().to_degrees();
    }
    Vec2::ZERO
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::input::pen::PenTilt;

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

    #[test]
    fn preview_tilt_uses_winit_plane_angles() {
        let data = PenData {
            tilt: Some(PenTilt { x: 30, y: 40 }),
            ..Default::default()
        };
        let projection = Vec2::new(30.0_f32.to_radians().tan(), 40.0_f32.to_radians().tan());
        let expected = projection.normalize_or_zero() * projection.length().atan().to_degrees();

        assert!(pen_tilt(&data).abs_diff_eq(expected, 0.0001));
    }

    #[test]
    fn shift_drag_sizes_right_and_up_with_limits() {
        let origin = Vec2::new(100.0, 100.0);
        assert_eq!(
            dragged_brush_size(20.0, origin, Vec2::new(120.0, 100.0)),
            27.0
        );
        assert_eq!(
            dragged_brush_size(20.0, origin, Vec2::new(100.0, 80.0)),
            27.0
        );
        assert_eq!(
            dragged_brush_size(20.0, origin, Vec2::new(-1_000.0, 1_000.0)),
            MIN_BRUSH_SIZE
        );
        assert_eq!(
            dragged_brush_size(20.0, origin, Vec2::new(1_000.0, -1_000.0)),
            MAX_BRUSH_SIZE
        );
    }

    #[test]
    fn vector_surface_is_created_once_and_reused() {
        let mut document = VectorStrokeDocument::default();
        let mut session = VectorSession::default();

        let first = ensure_vector_surface(&mut document, &mut session);
        let second = ensure_vector_surface(&mut document, &mut session);

        assert_eq!(first, second);
        assert_eq!(document.canvases().len(), 1);
        assert_eq!(document.canvases()[0].layers.len(), 1);
    }

    #[test]
    fn vector_eraser_size_is_exposed_as_a_diameter() {
        let mut settings = VectorStrokeSettings::default();
        set_vector_tool_size(Tool::Eraser, &mut settings, 42.0);

        assert_eq!(settings.eraser_radius, 21.0);
        assert_eq!(vector_tool_size(Tool::Eraser, &settings), 42.0);
    }

    #[test]
    fn vector_pressure_preset_has_a_wide_size_range() {
        let settings = vector_renderer_settings();
        let style = &settings.pen_style;

        assert_eq!(style.width_at(1.0), 18.0);
        assert!(style.width_at(0.2) < 2.2);
        assert_eq!(style.width_at(0.0), 18.0 * VECTOR_MIN_WIDTH_FACTOR);
    }
}
