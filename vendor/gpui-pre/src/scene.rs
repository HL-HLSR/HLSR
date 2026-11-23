// todo("windows"): remove
#![cfg_attr(windows, allow(dead_code))]

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    AtlasTextureId, AtlasTile, Background, Bounds, ContentMask, Corners, Edges, Hsla, Path,
    PathTiles, Pixels, Point, Radians, ScaledPixels, Size, TimedTransformation,
    bounds_tree::BoundsTree,
};
use smallvec::SmallVec;
use std::{
    collections::VecDeque, fmt::Debug, iter::Peekable, ops::Range, rc::Rc, slice, time::Duration,
};

#[expect(missing_docs)]
pub type DrawOrder = u32;

/// A boolean stored as a `u32` so that GPU-facing structs contain no
/// compiler-inserted padding bytes, which would be undefined behavior to
/// reinterpret as `&[u8]` when writing instance buffers. Guaranteed to be
/// `0` or `1` by construction; shaders read it as a `u32`/`uint`.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
#[repr(transparent)]
pub struct PaddedBool32(u32);

impl From<bool> for PaddedBool32 {
    fn from(value: bool) -> Self {
        PaddedBool32(value as u32)
    }
}

impl PaddedBool32 {
    /// The wrapped boolean.
    pub fn as_bool(self) -> bool {
        self.0 != 0
    }
}

enum LayerItem {
    Range(Range<usize>),
    /// HLSR: примитив, преобразованный активным `PaintTransform` (`paint_operations` хранит исходный).
    Transformed(Primitive),
}

struct Layer {
    primitives: SmallVec<[LayerItem; 1]>,
    bounds: Bounds<ScaledPixels>,
    max_kind: PrimitiveKind,
    max_shared_kind: PrimitiveKind,
}

/// HLSR: преобразование сцены при отрисовке поддерева (сдвиг, масштаб вокруг точки,
/// прозрачность) без перераскладки, перерастеризации шрифтов и срыва кэша видов.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PaintTransform {
    pub scale: f32,
    pub translate: Point<ScaledPixels>,
    pub opacity: f32,
    pub outer_mask: Bounds<ScaledPixels>,
}

impl PaintTransform {
    fn map_bounds(&self, bounds: Bounds<ScaledPixels>) -> Bounds<ScaledPixels> {
        Bounds::new(
            Point::new(
                ScaledPixels(bounds.origin.x.0 * self.scale + self.translate.x.0),
                ScaledPixels(bounds.origin.y.0 * self.scale + self.translate.y.0),
            ),
            Size::new(
                ScaledPixels(bounds.size.width.0 * self.scale),
                ScaledPixels(bounds.size.height.0 * self.scale),
            ),
        )
    }

    fn map_corners(&self, corners: Corners<ScaledPixels>) -> Corners<ScaledPixels> {
        Corners {
            top_left: ScaledPixels(corners.top_left.0 * self.scale),
            top_right: ScaledPixels(corners.top_right.0 * self.scale),
            bottom_right: ScaledPixels(corners.bottom_right.0 * self.scale),
            bottom_left: ScaledPixels(corners.bottom_left.0 * self.scale),
        }
    }

    fn map_edges(&self, edges: Edges<ScaledPixels>) -> Edges<ScaledPixels> {
        Edges {
            top: ScaledPixels(edges.top.0 * self.scale),
            right: ScaledPixels(edges.right.0 * self.scale),
            bottom: ScaledPixels(edges.bottom.0 * self.scale),
            left: ScaledPixels(edges.left.0 * self.scale),
        }
    }

    fn map_mask(&self, mask: ContentMask<ScaledPixels>) -> ContentMask<ScaledPixels> {
        let m = mask.bounds;
        let om = self.outer_mask;
        let eps = 0.5;
        let left = if (m.origin.x.0 - om.origin.x.0).abs() <= eps {
            om.origin.x.0
        } else {
            (m.origin.x.0 * self.scale + self.translate.x.0).max(om.origin.x.0)
        };
        let top = if (m.origin.y.0 - om.origin.y.0).abs() <= eps {
            om.origin.y.0
        } else {
            (m.origin.y.0 * self.scale + self.translate.y.0).max(om.origin.y.0)
        };
        let right = if (m.right().0 - om.right().0).abs() <= eps {
            om.right().0
        } else {
            (m.right().0 * self.scale + self.translate.x.0).min(om.right().0)
        };
        let bottom = if (m.bottom().0 - om.bottom().0).abs() <= eps {
            om.bottom().0
        } else {
            (m.bottom().0 * self.scale + self.translate.y.0).min(om.bottom().0)
        };
        ContentMask {
            bounds: Bounds::from_corners(
                Point::new(ScaledPixels(left), ScaledPixels(top)),
                Point::new(ScaledPixels(right.max(left)), ScaledPixels(bottom.max(top))),
            ),
        }
    }

    fn matrix(&self) -> TransformationMatrix {
        TransformationMatrix {
            rotation_scale: [[self.scale, 0.0], [0.0, self.scale]],
            translation: [self.translate.x.0, self.translate.y.0],
        }
    }

    fn apply_primitive(&self, primitive: &mut Primitive) {
        let opacity = self.opacity;
        match primitive {
            Primitive::Shadow(shadow) => {
                shadow.bounds = self.map_bounds(shadow.bounds);
                shadow.shape_bounds = self.map_bounds(shadow.shape_bounds);
                shadow.corner_radii = self.map_corners(shadow.corner_radii);
                shadow.shape_corner_radii = self.map_corners(shadow.shape_corner_radii);
                shadow.blur_radius = ScaledPixels(shadow.blur_radius.0 * self.scale);
                shadow.content_mask = self.map_mask(shadow.content_mask);
                if opacity != 1.0 {
                    shadow.color.a *= opacity;
                }
            }
            Primitive::Quad(quad) => {
                quad.bounds = self.map_bounds(quad.bounds);
                quad.corner_radii = self.map_corners(quad.corner_radii);
                quad.border_widths = self.map_edges(quad.border_widths);
                quad.content_mask = self.map_mask(quad.content_mask);
                if opacity != 1.0 {
                    quad.background = quad.background.opacity(opacity);
                    quad.border_color.a *= opacity;
                }
            }
            Primitive::Path(path) => {
                path.bounds = self.map_bounds(path.bounds);
                path.paint.bounds = self.map_bounds(path.paint.bounds);
                path.paint.content_mask = self.map_mask(path.paint.content_mask);
                if self.scale != 1.0 || self.translate.x.0 != 0.0 || self.translate.y.0 != 0.0 {
                    Rc::make_mut(&mut path.tiles).transform(self.scale, self.translate);
                }
                if opacity != 1.0 {
                    path.paint.color = path.paint.color.opacity(opacity);
                }
            }
            Primitive::Underline(underline) => {
                underline.bounds = self.map_bounds(underline.bounds);
                underline.thickness = ScaledPixels(underline.thickness.0 * self.scale);
                underline.content_mask = self.map_mask(underline.content_mask);
                if opacity != 1.0 {
                    underline.color.a *= opacity;
                }
            }
            Primitive::MonochromeSprite(sprite) => {
                if sprite.transformation == TransformationMatrix::unit() {
                    sprite.bounds = self.map_bounds(sprite.bounds);
                } else {
                    sprite.transformation = self.matrix().compose(sprite.transformation);
                }
                sprite.content_mask = self.map_mask(sprite.content_mask);
                if opacity != 1.0 {
                    sprite.color.a *= opacity;
                }
            }
            Primitive::SubpixelSprite(sprite) => {
                if sprite.transformation == TransformationMatrix::unit() {
                    sprite.bounds = self.map_bounds(sprite.bounds);
                } else {
                    sprite.transformation = self.matrix().compose(sprite.transformation);
                }
                sprite.content_mask = self.map_mask(sprite.content_mask);
                if opacity != 1.0 {
                    sprite.color.a *= opacity;
                }
            }
            Primitive::PolychromeSprite(sprite) => {
                sprite.bounds = self.map_bounds(sprite.bounds);
                sprite.corner_radii = self.map_corners(sprite.corner_radii);
                sprite.content_mask = self.map_mask(sprite.content_mask);
                if opacity != 1.0 {
                    sprite.opacity *= opacity;
                }
            }
            Primitive::Surface(surface) => {
                surface.bounds = self.map_bounds(surface.bounds);
                surface.content_mask = self.map_mask(surface.content_mask);
            }
        }
    }
}

#[derive(Default)]
#[expect(missing_docs)]
pub struct Scene {
    pub(crate) paint_operations: Vec<PaintOperation>,
    primitive_bounds: BoundsTree<ScaledPixels>,
    layer_stack: Vec<usize>,
    layers: VecDeque<Layer>,
    /// HLSR: активные преобразования отрисовки поддеревьев.
    paint_transforms: SmallVec<[PaintTransform; 2]>,
    pub shadows: Vec<Shadow>,
    pub quads: Vec<Quad>,
    pub paths: Vec<PaintedPath>,
    pub underlines: Vec<Underline>,
    pub monochrome_sprites: Vec<MonochromeSprite>,
    pub subpixel_sprites: Vec<SubpixelSprite>,
    pub polychrome_sprites: Vec<PolychromeSprite>,
    pub surfaces: Vec<PaintSurface>,
    /// HLSR: радиус скругления углов окна в физических px (0 — без маски).
    pub window_corner_radius: ScaledPixels,
    animations: Vec<SpriteAnimation>,
}

#[expect(missing_docs)]
impl Scene {
    pub fn clear(&mut self) {
        self.paint_operations.clear();
        self.primitive_bounds.clear();
        self.layer_stack.clear();
        self.layers.clear();
        self.paint_transforms.clear();
        self.paths.clear();
        self.shadows.clear();
        self.quads.clear();
        self.underlines.clear();
        self.monochrome_sprites.clear();
        self.subpixel_sprites.clear();
        self.polychrome_sprites.clear();
        self.surfaces.clear();
        self.window_corner_radius = ScaledPixels(0.0);
        self.animations.clear();
    }

    /// HLSR: положить преобразование отрисовки поддерева на стек сцены.
    pub(crate) fn push_paint_transform(&mut self, transform: PaintTransform) {
        self.paint_transforms.push(transform);
    }

    /// HLSR: снять преобразование отрисовки со стека сцены.
    pub(crate) fn pop_paint_transform(&mut self) {
        self.paint_transforms.pop();
    }

    /// HLSR: рамка `bounds` (в логических px) после активных преобразований отрисовки.
    pub(crate) fn transform_bounds(&self, bounds: Bounds<Pixels>, scale_factor: f32) -> Bounds<Pixels> {
        if self.paint_transforms.is_empty() {
            return bounds;
        }
        let mut scaled = bounds.scale(scale_factor);
        for transform in self.paint_transforms.iter().rev() {
            scaled = transform.map_bounds(scaled);
        }
        Bounds::new(
            Point::new(
                Pixels(scaled.origin.x.0 / scale_factor),
                Pixels(scaled.origin.y.0 / scale_factor),
            ),
            Size::new(
                Pixels(scaled.size.width.0 / scale_factor),
                Pixels(scaled.size.height.0 / scale_factor),
            ),
        )
    }

    /// Inserts a sprite that [`Self::advance_animations`] keeps moving. The
    /// sprite's color is taken as its color before the animation's alpha.
    pub(crate) fn insert_animated_sprite(
        &mut self,
        mut sprite: MonochromeSprite,
        mut animation: SpriteAnimation,
    ) {
        sprite.animation = SpriteAnimationId(self.animations.len() as u32);
        animation.raw_center = animation.center;
        animation.raw_base_color = sprite.color;
        animation.base_color = sprite.color;
        if !self.paint_transforms.is_empty() {
            let sf = animation.scale_factor;
            for transform in self.paint_transforms.iter().rev() {
                animation.center = Point::new(
                    Pixels(
                        (animation.center.x.0 * sf * transform.scale + transform.translate.x.0)
                            / sf,
                    ),
                    Pixels(
                        (animation.center.y.0 * sf * transform.scale + transform.translate.y.0)
                            / sf,
                    ),
                );
                if transform.opacity != 1.0 {
                    animation.base_color.a *= transform.opacity;
                }
            }
            // Анимированные спрайты перезаписывают transformation в advance_animations, поэтому
            // масштабируем их bounds напрямую (а transformation в advance_animations вращает вокруг
            // уже сдвинутого animation.center).
            sprite.transformation = TransformationMatrix::unit();
        }
        if self.try_insert_primitive(sprite.into()) {
            self.animations.push(animation);
        }
    }

    /// How long the window may wait between presents of this scene while only
    /// its animations need a frame: `None` when nothing in it animates, and
    /// zero when an animation wants every frame.
    pub(crate) fn animation_frame_interval(&self) -> Option<Duration> {
        self.animations
            .iter()
            .map(|animation| {
                animation
                    .transformation
                    .min_frame_interval()
                    .unwrap_or_default()
            })
            .min()
    }

    /// Moves every animated sprite to its pose at `elapsed` time on the app's
    /// synced animation clock, without the scene being rebuilt.
    pub(crate) fn advance_animations(&mut self, elapsed: Duration) {
        for animation in &self.animations {
            let Some(sprite) = self.monochrome_sprites.get_mut(animation.sprite_index) else {
                continue;
            };
            let pose = animation.transformation.at(elapsed);
            sprite.transformation = pose.into_matrix(animation.center, animation.scale_factor);
            sprite.color = animation.base_color.opacity(pose.alpha_factor());
        }
    }

    pub fn len(&self) -> usize {
        self.paint_operations.len()
    }

    pub fn push_layer(&mut self) {
        self.layer_stack.push(self.layers.len());
        self.layers.push_back(Layer {
            primitives: SmallVec::new(),
            bounds: Bounds::default(),
            max_kind: PrimitiveKind::Shadow,
            max_shared_kind: PrimitiveKind::Surface,
        });
        self.paint_operations.push(PaintOperation::StartLayer);
    }

    pub fn pop_layer(&mut self) {
        self.layer_stack.pop();
        if self.layer_stack.is_empty() {
            while let Some(layer) = self.layers.pop_front() {
                self.insert_layer(layer);
            }
        }
        self.paint_operations.push(PaintOperation::EndLayer);
    }

    fn insert_layer(&mut self, layer: Layer) {
        if layer.primitives.is_empty() {
            return;
        }
        let order =
            self.primitive_bounds
                .insert(layer.bounds, layer.max_kind, layer.max_shared_kind);
        for item in layer.primitives {
            match item {
                LayerItem::Range(range) => {
                    for index in range {
                        if let PaintOperation::Primitive(primitive) =
                            &mut self.paint_operations[index]
                        {
                            primitive.set_order(order);
                            let primitive = primitive.clone();
                            self.push_primitive(primitive, order);
                        }
                    }
                }
                LayerItem::Transformed(primitive) => self.push_primitive(primitive, order),
            }
        }
    }

    pub fn insert_primitive(&mut self, primitive: impl Into<Primitive>) {
        self.try_insert_primitive(primitive.into());
    }

    /// Returns whether the primitive was kept, rather than clipped away.
    fn try_insert_primitive(&mut self, mut primitive: Primitive) -> bool {
        let clipped_bounds = primitive.clipped_bounds();
        if clipped_bounds.is_empty() {
            return false;
        }

        if self.paint_transforms.is_empty() {
            let kind = primitive.kind();
            if let Some(&layer) = self.layer_stack.last()
                && let Some(layer) = self.layers.get_mut(layer)
            {
                layer.bounds = layer.bounds.join(&clipped_bounds);
                layer.max_kind = layer.max_kind.max(kind);
                layer.max_shared_kind = layer.max_shared_kind.min(kind.max_shared_kind());
                let index = self.paint_operations.len();
                match layer.primitives.last_mut() {
                    Some(LayerItem::Range(range)) if range.end == index => range.end += 1,
                    _ => layer.primitives.push(LayerItem::Range(index..index + 1)),
                }
            } else {
                let order = self
                    .primitive_bounds
                    .insert(clipped_bounds, kind, kind.max_shared_kind());
                primitive.set_order(order);
                self.push_primitive(primitive.clone(), order);
            }
            self.paint_operations
                .push(PaintOperation::Primitive(primitive));
            return true;
        }

        // HLSR: `paint_operations` всегда хранит исходный примитив (для `Scene::replay`
        // кэшированных видов без повторного преобразования и потери примитивов, вышедших за край).
        primitive.set_order(0);
        self.paint_operations
            .push(PaintOperation::Primitive(primitive.clone()));
        let visible_alpha = self
            .paint_transforms
            .iter()
            .fold(1.0f32, |a, t| a * t.opacity);
        if visible_alpha <= 0.0 {
            return true;
        }
        let mut transformed = primitive;
        for transform in self.paint_transforms.iter().rev() {
            transform.apply_primitive(&mut transformed);
        }
        let clipped_bounds = transformed.clipped_bounds();
        if clipped_bounds.is_empty() {
            return true;
        }
        let kind = transformed.kind();
        if let Some(&layer) = self.layer_stack.last()
            && let Some(layer) = self.layers.get_mut(layer)
        {
            layer.bounds = layer.bounds.join(&clipped_bounds);
            layer.max_kind = layer.max_kind.max(kind);
            layer.max_shared_kind = layer.max_shared_kind.min(kind.max_shared_kind());
            layer.primitives.push(LayerItem::Transformed(transformed));
        } else {
            let order = self
                .primitive_bounds
                .insert(clipped_bounds, kind, kind.max_shared_kind());
            self.push_primitive(transformed, order);
        }
        true
    }

    fn push_primitive(&mut self, primitive: Primitive, order: DrawOrder) {
        match primitive {
            Primitive::Shadow(shadow) => self.shadows.push(Shadow { order, ..shadow }),
            Primitive::Quad(quad) => self.quads.push(Quad { order, ..quad }),
            Primitive::Path(mut path) => {
                path.order = order;
                self.paths.push(path);
            }
            Primitive::Underline(underline) => {
                self.underlines.push(Underline { order, ..underline })
            }
            Primitive::MonochromeSprite(sprite) => self
                .monochrome_sprites
                .push(MonochromeSprite { order, ..sprite }),
            Primitive::SubpixelSprite(sprite) => self
                .subpixel_sprites
                .push(SubpixelSprite { order, ..sprite }),
            Primitive::PolychromeSprite(sprite) => self
                .polychrome_sprites
                .push(PolychromeSprite { order, ..sprite }),
            Primitive::Surface(mut surface) => {
                surface.order = order;
                self.surfaces.push(surface);
            }
        }
    }

    /// HLSR: локальный порядок примитивов для среза `paint_operations`, если при первой отрисовке
    /// поддерева действовал `PaintTransform` и исходные `order` ещё не посчитаны.
    fn compute_local_orders(ops: &[PaintOperation]) -> Vec<DrawOrder> {
        struct LocalLayer {
            indices: Vec<usize>,
            bounds: Bounds<ScaledPixels>,
            max_kind: PrimitiveKind,
            max_shared_kind: PrimitiveKind,
        }
        let mut tree: BoundsTree<ScaledPixels> = BoundsTree::default();
        let mut stack: Vec<usize> = Vec::new();
        let mut layers: VecDeque<LocalLayer> = VecDeque::new();
        let mut orders = vec![0u32; ops.len()];
        for (idx, op) in ops.iter().enumerate() {
            match op {
                PaintOperation::StartLayer => {
                    stack.push(layers.len());
                    layers.push_back(LocalLayer {
                        indices: Vec::new(),
                        bounds: Bounds::default(),
                        max_kind: PrimitiveKind::Shadow,
                        max_shared_kind: PrimitiveKind::Surface,
                    });
                }
                PaintOperation::EndLayer => {
                    stack.pop();
                    if stack.is_empty() {
                        while let Some(layer) = layers.pop_front() {
                            if layer.indices.is_empty() {
                                continue;
                            }
                            let order = tree.insert(
                                layer.bounds,
                                layer.max_kind,
                                layer.max_shared_kind,
                            );
                            for i in layer.indices {
                                orders[i] = order;
                            }
                        }
                    }
                }
                PaintOperation::Primitive(primitive) => {
                    let cb = primitive.clipped_bounds();
                    if cb.is_empty() {
                        orders[idx] = 1;
                        continue;
                    }
                    let kind = primitive.kind();
                    if let Some(&layer_idx) = stack.last()
                        && let Some(layer) = layers.get_mut(layer_idx)
                    {
                        layer.bounds = layer.bounds.join(&cb);
                        layer.max_kind = layer.max_kind.max(kind);
                        layer.max_shared_kind = layer.max_shared_kind.min(kind.max_shared_kind());
                        layer.indices.push(idx);
                    } else {
                        orders[idx] = tree.insert(cb, kind, kind.max_shared_kind());
                    }
                }
            }
        }
        orders
    }

    pub fn replay(&mut self, range: Range<usize>, prev_scene: &Scene) {
        let ops = &prev_scene.paint_operations[range];
        if ops.is_empty() {
            return;
        }
        // HLSR: внутри активного `paint_layer` используем исходный путь со слоями;
        // в обычном случае (закэшированные виды и их свайпы/переходы) — O(1) вставка
        // в `BoundsTree` с сохранением локальных относительных `DrawOrder`.
        if !self.layer_stack.is_empty() {
            for operation in ops {
                match operation {
                    PaintOperation::Primitive(primitive) => {
                        if let Primitive::MonochromeSprite(sprite) = primitive
                            && let Some(animation) =
                                prev_scene.animations.get(sprite.animation.0 as usize)
                        {
                            let mut animation = *animation;
                            animation.center = animation.raw_center;
                            animation.base_color = animation.raw_base_color;
                            self.insert_animated_sprite(*sprite, animation);
                        } else {
                            self.insert_primitive(primitive.clone());
                        }
                    }
                    PaintOperation::StartLayer => self.push_layer(),
                    PaintOperation::EndLayer => self.pop_layer(),
                }
            }
            return;
        }

        let mut min_order = u32::MAX;
        let mut max_order = 0u32;
        let mut max_kind = PrimitiveKind::Shadow;
        let mut needs_local = false;
        let mut has_primitives = false;
        for op in ops {
            if let PaintOperation::Primitive(p) = op {
                has_primitives = true;
                let o = p.order();
                if o == 0 {
                    needs_local = true;
                } else {
                    min_order = min_order.min(o);
                    max_order = max_order.max(o);
                }
                max_kind = max_kind.max(p.kind());
            }
        }
        if !has_primitives {
            for op in ops {
                match op {
                    PaintOperation::StartLayer => {
                        self.paint_operations.push(PaintOperation::StartLayer);
                    }
                    PaintOperation::EndLayer => {
                        self.paint_operations.push(PaintOperation::EndLayer);
                    }
                    PaintOperation::Primitive(_) => {}
                }
            }
            return;
        }

        let local_orders = if needs_local {
            let computed = Self::compute_local_orders(ops);
            min_order = u32::MAX;
            max_order = 0;
            for (idx, op) in ops.iter().enumerate() {
                if matches!(op, PaintOperation::Primitive(_)) {
                    let o = computed[idx].max(1);
                    min_order = min_order.min(o);
                    max_order = max_order.max(o);
                }
            }
            Some(computed)
        } else {
            None
        };

        let has_transform = !self.paint_transforms.is_empty();
        let visible_alpha = if has_transform {
            self.paint_transforms
                .iter()
                .fold(1.0f32, |a, t| a * t.opacity)
        } else {
            1.0
        };

        let op_start = self.paint_operations.len();
        self.paint_operations.reserve(ops.len());
        let s0 = self.shadows.len();
        let q0 = self.quads.len();
        let p0 = self.paths.len();
        let u0 = self.underlines.len();
        let m0 = self.monochrome_sprites.len();
        let sub0 = self.subpixel_sprites.len();
        let poly0 = self.polychrome_sprites.len();
        let surf0 = self.surfaces.len();
        let mut union_bounds: Option<Bounds<ScaledPixels>> = None;

        for (idx, op) in ops.iter().enumerate() {
            match op {
                PaintOperation::StartLayer => {
                    self.paint_operations.push(PaintOperation::StartLayer);
                }
                PaintOperation::EndLayer => {
                    self.paint_operations.push(PaintOperation::EndLayer);
                }
                PaintOperation::Primitive(primitive) => {
                    let local_order = local_orders
                        .as_ref()
                        .map_or_else(|| primitive.order(), |v| v[idx].max(1));
                    let rel_order = local_order - min_order;

                    if let Primitive::MonochromeSprite(sprite) = primitive
                        && let Some(prev_anim) =
                            prev_scene.animations.get(sprite.animation.0 as usize)
                    {
                        let mut stored_sprite = *sprite;
                        let new_anim_id = SpriteAnimationId(self.animations.len() as u32);
                        stored_sprite.animation = new_anim_id;
                        stored_sprite.order = local_order;
                        self.paint_operations
                            .push(PaintOperation::Primitive(Primitive::MonochromeSprite(
                                stored_sprite,
                            )));

                        let mut animation = *prev_anim;
                        animation.center = animation.raw_center;
                        animation.base_color = animation.raw_base_color;
                        animation.sprite_index = usize::MAX;
                        if has_transform {
                            let sf = animation.scale_factor;
                            for transform in self.paint_transforms.iter().rev() {
                                animation.center = Point::new(
                                    Pixels(
                                        (animation.center.x.0 * sf * transform.scale
                                            + transform.translate.x.0)
                                            / sf,
                                    ),
                                    Pixels(
                                        (animation.center.y.0 * sf * transform.scale
                                            + transform.translate.y.0)
                                            / sf,
                                    ),
                                );
                                if transform.opacity != 1.0 {
                                    animation.base_color.a *= transform.opacity;
                                }
                            }
                        }
                        self.animations.push(animation);

                        if visible_alpha > 0.0 {
                            let mut emitted = stored_sprite;
                            if has_transform {
                                emitted.transformation = TransformationMatrix::unit();
                                let mut prim = Primitive::MonochromeSprite(emitted);
                                for transform in self.paint_transforms.iter().rev() {
                                    transform.apply_primitive(&mut prim);
                                }
                                let cb = prim.clipped_bounds();
                                if !cb.is_empty() {
                                    union_bounds = Some(match union_bounds {
                                        Some(b) => b.union(&cb),
                                        None => cb,
                                    });
                                    self.push_primitive(prim, rel_order);
                                }
                            } else {
                                let cb = Primitive::MonochromeSprite(emitted).clipped_bounds();
                                if !cb.is_empty() {
                                    union_bounds = Some(match union_bounds {
                                        Some(b) => b.union(&cb),
                                        None => cb,
                                    });
                                    self.monochrome_sprites.push(MonochromeSprite {
                                        order: rel_order,
                                        ..emitted
                                    });
                                }
                            }
                        }
                        continue;
                    }

                    let mut stored = primitive.clone();
                    stored.set_order(local_order);
                    self.paint_operations
                        .push(PaintOperation::Primitive(stored.clone()));

                    if visible_alpha <= 0.0 {
                        continue;
                    }
                    let emitted = if has_transform {
                        for transform in self.paint_transforms.iter().rev() {
                            transform.apply_primitive(&mut stored);
                        }
                        stored
                    } else {
                        stored
                    };
                    let cb = emitted.clipped_bounds();
                    if !cb.is_empty() {
                        union_bounds = Some(match union_bounds {
                            Some(b) => b.union(&cb),
                            None => cb,
                        });
                        self.push_primitive(emitted, rel_order);
                    }
                }
            }
        }

        if let Some(bounds) = union_bounds {
            let span = max_order - min_order;
            let base = self.primitive_bounds.reserve_range(bounds, span, max_kind);
            for s in &mut self.shadows[s0..] {
                s.order += base;
            }
            for q in &mut self.quads[q0..] {
                q.order += base;
            }
            for p in &mut self.paths[p0..] {
                p.order += base;
            }
            for u in &mut self.underlines[u0..] {
                u.order += base;
            }
            for m in &mut self.monochrome_sprites[m0..] {
                m.order += base;
            }
            for s in &mut self.subpixel_sprites[sub0..] {
                s.order += base;
            }
            for p in &mut self.polychrome_sprites[poly0..] {
                p.order += base;
            }
            for s in &mut self.surfaces[surf0..] {
                s.order += base;
            }
            if !has_transform {
                for op in &mut self.paint_operations[op_start..] {
                    if let PaintOperation::Primitive(p) = op {
                        let o = base + (p.order() - min_order);
                        p.set_order(o);
                    }
                }
            }
        }
    }

    pub fn finish(&mut self) {
        debug_assert!(self.layer_stack.is_empty(), "unbalanced scene layers");
        let keys = &mut Vec::new();
        sort_in_place(&mut self.shadows, keys, |shadow| (shadow.order, 0));
        sort_in_place(&mut self.quads, keys, |quad| (quad.order, 0));
        self.paths.sort_by_key(|path| path.order);
        sort_in_place(&mut self.underlines, keys, |underline| (underline.order, 0));
        sort_in_place(&mut self.monochrome_sprites, keys, |sprite| {
            (sprite.order, sprite.tile.texture_id.index)
        });
        sort_in_place(&mut self.subpixel_sprites, keys, |sprite| {
            (sprite.order, sprite.tile.texture_id.index)
        });
        sort_in_place(&mut self.polychrome_sprites, keys, |sprite| {
            (sprite.order, sprite.tile.texture_id.index)
        });
        self.surfaces.sort_by_key(|surface| surface.order);

        // Sorting moved the sprites, so animations learn where theirs ended up.
        if !self.animations.is_empty() {
            for (index, sprite) in self.monochrome_sprites.iter().enumerate() {
                if let Some(animation) = self.animations.get_mut(sprite.animation.0 as usize) {
                    animation.sprite_index = index;
                }
            }
        }
    }

    #[cfg_attr(
        all(
            any(target_os = "linux", target_os = "freebsd"),
            not(any(feature = "x11", feature = "wayland"))
        ),
        allow(dead_code)
    )]
    pub fn batches(&self) -> impl Iterator<Item = PrimitiveBatch> + '_ {
        BatchIterator {
            shadows_start: 0,
            shadows_iter: self.shadows.iter().peekable(),
            quads_start: 0,
            quads_iter: self.quads.iter().peekable(),
            paths_start: 0,
            paths_iter: self.paths.iter().peekable(),
            underlines_start: 0,
            underlines_iter: self.underlines.iter().peekable(),
            monochrome_sprites_start: 0,
            monochrome_sprites_iter: self.monochrome_sprites.iter().peekable(),
            subpixel_sprites_start: 0,
            subpixel_sprites_iter: self.subpixel_sprites.iter().peekable(),
            polychrome_sprites_start: 0,
            polychrome_sprites_iter: self.polychrome_sprites.iter().peekable(),
            surfaces_start: 0,
            surfaces_iter: self.surfaces.iter().peekable(),
        }
    }
}

/// Sorts small `(key, index)` pairs instead of the primitives themselves, then moves each
/// primitive into place once. Primitives are large, so a comparison sort that shuffles them
/// directly spends most of its time copying. Ties keep insertion order, as `sort_by_key`
/// does, because the index is part of what is sorted.
fn sort_in_place<T: Copy>(
    primitives: &mut [T],
    keys: &mut Vec<(u64, u32)>,
    key: impl Fn(&T) -> (u32, u32),
) {
    if primitives.is_sorted_by_key(&key) {
        return;
    }
    keys.clear();
    keys.extend(primitives.iter().enumerate().map(|(index, primitive)| {
        let (major, minor) = key(primitive);
        ((major as u64) << 32 | minor as u64, index as u32)
    }));
    keys.sort_unstable();

    // `keys[target].1` is the source index for `target`. Follow each cycle of that
    // permutation, marking visited slots by pointing them at themselves.
    for start in 0..primitives.len() {
        if keys[start].1 as usize == start {
            continue;
        }
        let displaced = primitives[start];
        let mut target = start;
        loop {
            let source = keys[target].1 as usize;
            keys[target].1 = target as u32;
            if source == start {
                primitives[target] = displaced;
                break;
            }
            primitives[target] = primitives[source];
            target = source;
        }
    }
}

/// The depth step between consecutive quads in a frame with `quad_count` quads.
pub fn quad_depth_scale(quad_count: usize) -> f32 {
    let denominator = (quad_count.min((1 << 24) - 1) + 1).next_power_of_two();
    1.0 / denominator as f32
}

/// Maps a quad's index in [`Scene::quads`] to depth, with greater values closer.
pub fn quad_depth(quad_id: u32, scale: f32) -> f32 {
    ((quad_id + 1) as f32 * scale).min(1.0)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Default)]
#[cfg_attr(
    all(
        any(target_os = "linux", target_os = "freebsd"),
        not(any(feature = "x11", feature = "wayland"))
    ),
    allow(dead_code)
)]
pub(crate) enum PrimitiveKind {
    Shadow,
    #[default]
    Quad,
    Path,
    Underline,
    MonochromeSprite,
    SubpixelSprite,
    PolychromeSprite,
    Surface,
}

impl PrimitiveKind {
    /// We guarantee that overlapping primitives are drawn in paint order.
    /// But, to improve GPU draw batching, the renderer sorts primitives that
    /// share a draw order by kind, then by atlas texture, and only then in
    /// paint order. So, a primitive can only share the draw order of the
    /// topmost primitive it overlaps if that primitive's kind is at most the
    /// one returned here. Otherwise, it needs a new draw order.
    fn max_shared_kind(self) -> Self {
        match self {
            Self::MonochromeSprite => Self::Underline,
            Self::SubpixelSprite => Self::MonochromeSprite,
            Self::PolychromeSprite => Self::SubpixelSprite,
            kind => kind,
        }
    }
}

pub(crate) enum PaintOperation {
    Primitive(Primitive),
    StartLayer,
    EndLayer,
}

#[derive(Clone)]
#[expect(missing_docs)]
pub enum Primitive {
    Shadow(Shadow),
    Quad(Quad),
    Path(PaintedPath),
    Underline(Underline),
    MonochromeSprite(MonochromeSprite),
    SubpixelSprite(SubpixelSprite),
    PolychromeSprite(PolychromeSprite),
    Surface(PaintSurface),
}

#[expect(missing_docs)]
impl Primitive {
    pub(crate) fn kind(&self) -> PrimitiveKind {
        match self {
            Primitive::Shadow(_) => PrimitiveKind::Shadow,
            Primitive::Quad(_) => PrimitiveKind::Quad,
            Primitive::Path(_) => PrimitiveKind::Path,
            Primitive::Underline(_) => PrimitiveKind::Underline,
            Primitive::MonochromeSprite(_) => PrimitiveKind::MonochromeSprite,
            Primitive::SubpixelSprite(_) => PrimitiveKind::SubpixelSprite,
            Primitive::PolychromeSprite(_) => PrimitiveKind::PolychromeSprite,
            Primitive::Surface(_) => PrimitiveKind::Surface,
        }
    }

    /// HLSR: сохранённый локальный `DrawOrder` примитива в `paint_operations` (0 — ещё не вычислен).
    pub(crate) fn order(&self) -> DrawOrder {
        match self {
            Primitive::Shadow(shadow) => shadow.order,
            Primitive::Quad(quad) => quad.order,
            Primitive::Path(path) => path.order,
            Primitive::Underline(underline) => underline.order,
            Primitive::MonochromeSprite(sprite) => sprite.order,
            Primitive::SubpixelSprite(sprite) => sprite.order,
            Primitive::PolychromeSprite(sprite) => sprite.order,
            Primitive::Surface(surface) => surface.order,
        }
    }

    /// HLSR: записать `DrawOrder` в примитив внутри `paint_operations`.
    pub(crate) fn set_order(&mut self, order: DrawOrder) {
        match self {
            Primitive::Shadow(shadow) => shadow.order = order,
            Primitive::Quad(quad) => quad.order = order,
            Primitive::Path(path) => path.order = order,
            Primitive::Underline(underline) => underline.order = order,
            Primitive::MonochromeSprite(sprite) => sprite.order = order,
            Primitive::SubpixelSprite(sprite) => sprite.order = order,
            Primitive::PolychromeSprite(sprite) => sprite.order = order,
            Primitive::Surface(surface) => surface.order = order,
        }
    }

    pub fn bounds(&self) -> &Bounds<ScaledPixels> {
        match self {
            Primitive::Shadow(shadow) => &shadow.bounds,
            Primitive::Quad(quad) => &quad.bounds,
            Primitive::Path(path) => &path.bounds,
            Primitive::Underline(underline) => &underline.bounds,
            Primitive::MonochromeSprite(sprite) => &sprite.bounds,
            Primitive::SubpixelSprite(sprite) => &sprite.bounds,
            Primitive::PolychromeSprite(sprite) => &sprite.bounds,
            Primitive::Surface(surface) => &surface.bounds,
        }
    }

    fn transformation(&self) -> TransformationMatrix {
        match self {
            Primitive::MonochromeSprite(sprite) => sprite.transformation,
            Primitive::SubpixelSprite(sprite) => sprite.transformation,
            _ => TransformationMatrix::unit(),
        }
    }

    fn clipped_bounds(&self) -> Bounds<ScaledPixels> {
        self.transformation()
            .transform_bounds(*self.bounds())
            .intersect(&self.content_mask().bounds)
    }

    pub fn content_mask(&self) -> &ContentMask<ScaledPixels> {
        match self {
            Primitive::Shadow(shadow) => &shadow.content_mask,
            Primitive::Quad(quad) => &quad.content_mask,
            Primitive::Path(path) => &path.paint.content_mask,
            Primitive::Underline(underline) => &underline.content_mask,
            Primitive::MonochromeSprite(sprite) => &sprite.content_mask,
            Primitive::SubpixelSprite(sprite) => &sprite.content_mask,
            Primitive::PolychromeSprite(sprite) => &sprite.content_mask,
            Primitive::Surface(surface) => &surface.content_mask,
        }
    }
}

#[cfg_attr(
    all(
        any(target_os = "linux", target_os = "freebsd"),
        not(any(feature = "x11", feature = "wayland"))
    ),
    allow(dead_code)
)]
struct BatchIterator<'a> {
    shadows_start: usize,
    shadows_iter: Peekable<slice::Iter<'a, Shadow>>,
    quads_start: usize,
    quads_iter: Peekable<slice::Iter<'a, Quad>>,
    paths_start: usize,
    paths_iter: Peekable<slice::Iter<'a, PaintedPath>>,
    underlines_start: usize,
    underlines_iter: Peekable<slice::Iter<'a, Underline>>,
    monochrome_sprites_start: usize,
    monochrome_sprites_iter: Peekable<slice::Iter<'a, MonochromeSprite>>,
    subpixel_sprites_start: usize,
    subpixel_sprites_iter: Peekable<slice::Iter<'a, SubpixelSprite>>,
    polychrome_sprites_start: usize,
    polychrome_sprites_iter: Peekable<slice::Iter<'a, PolychromeSprite>>,
    surfaces_start: usize,
    surfaces_iter: Peekable<slice::Iter<'a, PaintSurface>>,
}

impl<'a> Iterator for BatchIterator<'a> {
    type Item = PrimitiveBatch;

    fn next(&mut self) -> Option<Self::Item> {
        let mut orders_and_kinds = [
            (
                self.shadows_iter.peek().map(|s| s.order),
                PrimitiveKind::Shadow,
            ),
            (self.quads_iter.peek().map(|q| q.order), PrimitiveKind::Quad),
            (
                self.paths_iter.peek().map(|path| path.order),
                PrimitiveKind::Path,
            ),
            (
                self.underlines_iter.peek().map(|u| u.order),
                PrimitiveKind::Underline,
            ),
            (
                self.monochrome_sprites_iter.peek().map(|s| s.order),
                PrimitiveKind::MonochromeSprite,
            ),
            (
                self.subpixel_sprites_iter.peek().map(|s| s.order),
                PrimitiveKind::SubpixelSprite,
            ),
            (
                self.polychrome_sprites_iter.peek().map(|s| s.order),
                PrimitiveKind::PolychromeSprite,
            ),
            (
                self.surfaces_iter.peek().map(|s| s.order),
                PrimitiveKind::Surface,
            ),
        ];
        orders_and_kinds.sort_by_key(|(order, kind)| (order.unwrap_or(u32::MAX), *kind));

        let first = orders_and_kinds[0];
        let second = orders_and_kinds[1];
        let (batch_kind, max_order_and_kind) = if first.0.is_some() {
            (first.1, (second.0.unwrap_or(u32::MAX), second.1))
        } else {
            return None;
        };

        match batch_kind {
            PrimitiveKind::Shadow => {
                let shadows_start = self.shadows_start;
                let mut shadows_end = shadows_start + 1;
                self.shadows_iter.next();
                while self
                    .shadows_iter
                    .next_if(|shadow| (shadow.order, batch_kind) < max_order_and_kind)
                    .is_some()
                {
                    shadows_end += 1;
                }
                self.shadows_start = shadows_end;
                Some(PrimitiveBatch::Shadows(shadows_start..shadows_end))
            }
            PrimitiveKind::Quad => {
                let quads_start = self.quads_start;
                let mut quads_end = quads_start + 1;
                self.quads_iter.next();
                while self
                    .quads_iter
                    .next_if(|quad| (quad.order, batch_kind) < max_order_and_kind)
                    .is_some()
                {
                    quads_end += 1;
                }
                self.quads_start = quads_end;
                Some(PrimitiveBatch::Quads(quads_start..quads_end))
            }
            PrimitiveKind::Path => {
                let paths_start = self.paths_start;
                let mut paths_end = paths_start + 1;
                self.paths_iter.next();
                while self
                    .paths_iter
                    .next_if(|path| (path.order, batch_kind) < max_order_and_kind)
                    .is_some()
                {
                    paths_end += 1;
                }
                self.paths_start = paths_end;
                Some(PrimitiveBatch::Paths(paths_start..paths_end))
            }
            PrimitiveKind::Underline => {
                let underlines_start = self.underlines_start;
                let mut underlines_end = underlines_start + 1;
                self.underlines_iter.next();
                while self
                    .underlines_iter
                    .next_if(|underline| (underline.order, batch_kind) < max_order_and_kind)
                    .is_some()
                {
                    underlines_end += 1;
                }
                self.underlines_start = underlines_end;
                Some(PrimitiveBatch::Underlines(underlines_start..underlines_end))
            }
            PrimitiveKind::MonochromeSprite => {
                let texture_id = self.monochrome_sprites_iter.peek().unwrap().tile.texture_id;
                let sprites_start = self.monochrome_sprites_start;
                let mut sprites_end = sprites_start + 1;
                self.monochrome_sprites_iter.next();
                while self
                    .monochrome_sprites_iter
                    .next_if(|sprite| {
                        (sprite.order, batch_kind) < max_order_and_kind
                            && sprite.tile.texture_id == texture_id
                    })
                    .is_some()
                {
                    sprites_end += 1;
                }
                self.monochrome_sprites_start = sprites_end;
                Some(PrimitiveBatch::MonochromeSprites {
                    texture_id,
                    range: sprites_start..sprites_end,
                })
            }
            PrimitiveKind::SubpixelSprite => {
                let texture_id = self.subpixel_sprites_iter.peek().unwrap().tile.texture_id;
                let sprites_start = self.subpixel_sprites_start;
                let mut sprites_end = sprites_start + 1;
                self.subpixel_sprites_iter.next();
                while self
                    .subpixel_sprites_iter
                    .next_if(|sprite| {
                        (sprite.order, batch_kind) < max_order_and_kind
                            && sprite.tile.texture_id == texture_id
                    })
                    .is_some()
                {
                    sprites_end += 1;
                }
                self.subpixel_sprites_start = sprites_end;
                Some(PrimitiveBatch::SubpixelSprites {
                    texture_id,
                    range: sprites_start..sprites_end,
                })
            }
            PrimitiveKind::PolychromeSprite => {
                let texture_id = self.polychrome_sprites_iter.peek().unwrap().tile.texture_id;
                let sprites_start = self.polychrome_sprites_start;
                let mut sprites_end = sprites_start + 1;
                self.polychrome_sprites_iter.next();
                while self
                    .polychrome_sprites_iter
                    .next_if(|sprite| {
                        (sprite.order, batch_kind) < max_order_and_kind
                            && sprite.tile.texture_id == texture_id
                    })
                    .is_some()
                {
                    sprites_end += 1;
                }
                self.polychrome_sprites_start = sprites_end;
                Some(PrimitiveBatch::PolychromeSprites {
                    texture_id,
                    range: sprites_start..sprites_end,
                })
            }
            PrimitiveKind::Surface => {
                let surfaces_start = self.surfaces_start;
                let mut surfaces_end = surfaces_start + 1;
                self.surfaces_iter.next();
                while self
                    .surfaces_iter
                    .next_if(|surface| (surface.order, batch_kind) < max_order_and_kind)
                    .is_some()
                {
                    surfaces_end += 1;
                }
                self.surfaces_start = surfaces_end;
                Some(PrimitiveBatch::Surfaces(surfaces_start..surfaces_end))
            }
        }
    }
}

#[derive(Debug)]
#[cfg_attr(
    all(
        any(target_os = "linux", target_os = "freebsd"),
        not(any(feature = "x11", feature = "wayland"))
    ),
    allow(dead_code)
)]
#[allow(missing_docs)]
pub enum PrimitiveBatch {
    Shadows(Range<usize>),
    Quads(Range<usize>),
    Paths(Range<usize>),
    Underlines(Range<usize>),
    MonochromeSprites {
        texture_id: AtlasTextureId,
        range: Range<usize>,
    },
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    SubpixelSprites {
        texture_id: AtlasTextureId,
        range: Range<usize>,
    },
    PolychromeSprites {
        texture_id: AtlasTextureId,
        range: Range<usize>,
    },
    Surfaces(Range<usize>),
}

impl PrimitiveBatch {
    #[expect(missing_docs)]
    pub fn label(&self) -> String {
        match self {
            Self::Shadows(range) => format!("shadows ({})", range.len()),
            Self::Quads(range) => format!("quads ({})", range.len()),
            Self::Paths(range) => format!("paths ({})", range.len()),
            Self::Underlines(range) => format!("underlines ({})", range.len()),
            Self::MonochromeSprites { texture_id, range } => {
                format!(
                    "monochrome sprites ({}) on atlas {}",
                    range.len(),
                    texture_id.index
                )
            }
            Self::SubpixelSprites { texture_id, range } => {
                format!(
                    "subpixel sprites ({}) on atlas {}",
                    range.len(),
                    texture_id.index
                )
            }
            Self::PolychromeSprites { texture_id, range } => {
                format!(
                    "polychrome sprites ({}) on atlas {}",
                    range.len(),
                    texture_id.index
                )
            }
            Self::Surfaces(range) => format!("surfaces ({})", range.len()),
        }
    }
}

#[derive(Default, Debug, Copy, Clone)]
#[repr(C)]
#[expect(missing_docs)]
pub struct Quad {
    pub order: DrawOrder,
    pub border_style: BorderStyle,
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub background: Background,
    pub border_color: Hsla,
    pub corner_radii: Corners<ScaledPixels>,
    pub border_widths: Edges<ScaledPixels>,
}

impl From<Quad> for Primitive {
    fn from(quad: Quad) -> Self {
        Primitive::Quad(quad)
    }
}

#[derive(Debug, Copy, Clone)]
#[repr(C)]
#[expect(missing_docs)]
pub struct Underline {
    pub order: DrawOrder,
    pub pad: u32, // align to 8 bytes
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub color: Hsla,
    pub thickness: ScaledPixels,
    pub wavy: PaddedBool32,
}

impl From<Underline> for Primitive {
    fn from(underline: Underline) -> Self {
        Primitive::Underline(underline)
    }
}

#[derive(Debug, Copy, Clone)]
#[repr(C)]
#[expect(missing_docs)]
pub struct Shadow {
    pub order: DrawOrder,
    pub blur_radius: ScaledPixels,
    pub bounds: Bounds<ScaledPixels>,
    pub corner_radii: Corners<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub color: Hsla,
    pub shape_bounds: Bounds<ScaledPixels>,
    pub shape_corner_radii: Corners<ScaledPixels>,
    pub inset: PaddedBool32,
    pub pad: u32, // align to 8 bytes
}

impl From<Shadow> for Primitive {
    fn from(shadow: Shadow) -> Self {
        Primitive::Shadow(shadow)
    }
}

/// The style of a border.
#[derive(Default, Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[repr(C)]
pub enum BorderStyle {
    /// A solid border.
    #[default]
    Solid = 0,
    /// A dashed border.
    Dashed = 1,
}

/// A data type representing a 2 dimensional transformation that can be applied to an element.
#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct TransformationMatrix {
    /// 2x2 matrix containing rotation and scale,
    /// stored row-major
    pub rotation_scale: [[f32; 2]; 2],
    /// translation vector
    pub translation: [f32; 2],
}

impl Eq for TransformationMatrix {}

impl TransformationMatrix {
    /// The unit matrix, has no effect.
    pub fn unit() -> Self {
        Self {
            rotation_scale: [[1.0, 0.0], [0.0, 1.0]],
            translation: [0.0, 0.0],
        }
    }

    /// Move the origin by a given point
    pub fn translate(self, point: Point<ScaledPixels>) -> Self {
        self.compose(Self {
            rotation_scale: [[1.0, 0.0], [0.0, 1.0]],
            translation: [point.x.0, point.y.0],
        })
    }

    /// Clockwise rotation in radians around the origin
    pub fn rotate(self, angle: Radians) -> Self {
        self.compose(Self {
            rotation_scale: [
                [angle.0.cos(), -angle.0.sin()],
                [angle.0.sin(), angle.0.cos()],
            ],
            translation: [0.0, 0.0],
        })
    }

    /// Scale around the origin
    pub fn scale(self, size: Size<f32>) -> Self {
        self.compose(Self {
            rotation_scale: [[size.width, 0.0], [0.0, size.height]],
            translation: [0.0, 0.0],
        })
    }

    /// Perform matrix multiplication with another transformation
    /// to produce a new transformation that is the result of
    /// applying both transformations: first, `other`, then `self`.
    #[inline]
    pub fn compose(self, other: TransformationMatrix) -> TransformationMatrix {
        if other == Self::unit() {
            return self;
        }
        // Perform matrix multiplication
        TransformationMatrix {
            rotation_scale: [
                [
                    self.rotation_scale[0][0] * other.rotation_scale[0][0]
                        + self.rotation_scale[0][1] * other.rotation_scale[1][0],
                    self.rotation_scale[0][0] * other.rotation_scale[0][1]
                        + self.rotation_scale[0][1] * other.rotation_scale[1][1],
                ],
                [
                    self.rotation_scale[1][0] * other.rotation_scale[0][0]
                        + self.rotation_scale[1][1] * other.rotation_scale[1][0],
                    self.rotation_scale[1][0] * other.rotation_scale[0][1]
                        + self.rotation_scale[1][1] * other.rotation_scale[1][1],
                ],
            ],
            translation: [
                self.translation[0]
                    + self.rotation_scale[0][0] * other.translation[0]
                    + self.rotation_scale[0][1] * other.translation[1],
                self.translation[1]
                    + self.rotation_scale[1][0] * other.translation[0]
                    + self.rotation_scale[1][1] * other.translation[1],
            ],
        }
    }

    /// The smallest bounds that contain `bounds` once it is transformed.
    pub(crate) fn transform_bounds(&self, bounds: Bounds<ScaledPixels>) -> Bounds<ScaledPixels> {
        if *self == Self::unit() {
            return bounds;
        }
        let [[xx, xy], [yx, yy]] = self.rotation_scale;
        let [translate_x, translate_y] = self.translation;
        let corners = [
            bounds.origin,
            bounds.top_right(),
            bounds.bottom_left(),
            bounds.bottom_right(),
        ]
        .map(|corner| {
            Point::new(
                ScaledPixels(xx * corner.x.0 + xy * corner.y.0 + translate_x),
                ScaledPixels(yx * corner.x.0 + yy * corner.y.0 + translate_y),
            )
        });
        let [first, rest @ ..] = corners;
        let top_left = rest.iter().fold(first, |min, corner| min.min(corner));
        let bottom_right = rest.iter().fold(first, |max, corner| max.max(corner));
        Bounds::from_corners(top_left, bottom_right)
    }

    /// Apply transformation to a point, mainly useful for debugging
    pub fn apply(&self, point: Point<Pixels>) -> Point<Pixels> {
        let input = [point.x.0, point.y.0];
        let mut output = self.translation;
        for (i, output_cell) in output.iter_mut().enumerate() {
            for (k, input_cell) in input.iter().enumerate() {
                *output_cell += self.rotation_scale[i][k] * *input_cell;
            }
        }
        Point::new(output[0].into(), output[1].into())
    }
}

impl Default for TransformationMatrix {
    fn default() -> Self {
        Self::unit()
    }
}

#[derive(Copy, Clone, Debug)]
#[repr(C)]
#[expect(missing_docs)]
pub struct MonochromeSprite {
    pub order: DrawOrder,
    pub animation: SpriteAnimationId,
    pub tile: AtlasTile,
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub color: Hsla,
    pub transformation: TransformationMatrix,
}

/// Refers to an animation of the scene a sprite is inserted into.
///
/// A single `u32` so it fits the sprite's existing padding and leaves the
/// instance layout unchanged on every backend; renderers ignore it.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(transparent)]
pub struct SpriteAnimationId(u32);

impl SpriteAnimationId {
    /// No animation.
    pub const NONE: Self = Self(u32::MAX);
}

impl Default for SpriteAnimationId {
    fn default() -> Self {
        Self::NONE
    }
}

/// A [`TimedTransformation`] the window applies to a sprite right before
/// every present, so the scene can be presented again without being redrawn.
#[derive(Copy, Clone, Debug)]
pub(crate) struct SpriteAnimation {
    transformation: TimedTransformation,
    center: Point<Pixels>,
    raw_center: Point<Pixels>,
    scale_factor: f32,
    base_color: Hsla,
    raw_base_color: Hsla,
    sprite_index: usize,
}

impl SpriteAnimation {
    pub(crate) fn new(
        transformation: TimedTransformation,
        center: Point<Pixels>,
        scale_factor: f32,
    ) -> Self {
        Self {
            transformation,
            center,
            raw_center: center,
            scale_factor,
            base_color: Hsla::default(),
            raw_base_color: Hsla::default(),
            sprite_index: usize::MAX,
        }
    }
}

impl From<MonochromeSprite> for Primitive {
    fn from(sprite: MonochromeSprite) -> Self {
        Primitive::MonochromeSprite(sprite)
    }
}

#[derive(Copy, Clone, Debug)]
#[repr(C)]
#[expect(missing_docs)]
pub struct SubpixelSprite {
    pub order: DrawOrder,
    pub pad: u32, // align to 8 bytes
    pub tile: AtlasTile,
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub color: Hsla,
    pub transformation: TransformationMatrix,
}

impl From<SubpixelSprite> for Primitive {
    fn from(sprite: SubpixelSprite) -> Self {
        Primitive::SubpixelSprite(sprite)
    }
}

#[derive(Copy, Clone, Debug)]
#[repr(C)]
#[expect(missing_docs)]
pub struct PolychromeSprite {
    pub order: DrawOrder,
    pub pad: u32,
    pub tile: AtlasTile,
    pub grayscale: PaddedBool32,
    pub opacity: f32,
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub corner_radii: Corners<ScaledPixels>,
}

impl From<PolychromeSprite> for Primitive {
    fn from(sprite: PolychromeSprite) -> Self {
        Primitive::PolychromeSprite(sprite)
    }
}

#[derive(Clone, Debug)]
#[allow(missing_docs)]
pub struct PaintSurface {
    pub order: DrawOrder,
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    pub image_buffer: objc2_core_foundation::CFRetained<objc2_core_video::CVPixelBuffer>,
}

impl From<PaintSurface> for Primitive {
    fn from(surface: PaintSurface) -> Self {
        Primitive::Surface(surface)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(C)]
#[expect(missing_docs)]
pub struct PathCurve {
    pub p0: Point<f32>,
    pub p1: Point<f32>,
    pub a: Point<f32>,
    pub b: Point<f32>,
    pub sign: f32,
    pub pad: u32, // align to 8 bytes
}

#[derive(Clone, Copy, Debug)]
#[repr(C)]
#[expect(missing_docs)]
pub struct PathPaint {
    /// The path's bounds, in device pixels.
    pub bounds: Bounds<ScaledPixels>,
    pub content_mask: ContentMask<ScaledPixels>,
    pub color: Background,
    pub even_odd: PaddedBool32,
    pub pad: u32, // align to 8 bytes
}

#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(C)]
#[expect(missing_docs)]
pub struct PathTile {
    pub paint: u32,
    pub curve_start: u32,
    pub curve_count: u32,
    pub backdrop: i32,
    pub top_left: Point<f32>,
    pub bottom_right: Point<f32>,
    pub reference: Point<f32>,
    pub curves_top_left: Point<f32>,
    pub curves_bottom_right: Point<f32>,
}

#[derive(Clone, Debug)]
#[expect(missing_docs)]
pub struct PaintedPath {
    order: DrawOrder,
    bounds: Bounds<ScaledPixels>,
    tiles: Rc<PathTiles>,
    paint: PathPaint,
}

#[expect(missing_docs)]
impl PaintedPath {
    /// Bins `path` into tiles, to be filled with `color` within `content_mask`.
    pub fn new(
        path: Path,
        scale_factor: f32,
        content_mask: ContentMask<Pixels>,
        color: Background,
    ) -> Self {
        let content_mask = content_mask.scale(scale_factor);
        let mut tiles = path.bin(scale_factor);
        tiles.retain_visible(content_mask.bounds);
        let paint = PathPaint {
            bounds: tiles.bounds(),
            content_mask,
            color,
            even_odd: path.even_odd().into(),
            pad: 0,
        };
        Self {
            order: 0,
            bounds: tiles.painted_bounds(),
            tiles: Rc::new(tiles),
            paint,
        }
    }

    pub fn tile_count(&self) -> usize {
        self.tiles.tiles().len()
    }

    pub fn append_to(
        &self,
        paints: &mut Vec<PathPaint>,
        curves: &mut Vec<PathCurve>,
        tiles: &mut Vec<PathTile>,
    ) {
        let paint_index = paints.len() as u32;
        let curve_start = curves.len() as u32;
        paints.push(self.paint);
        curves.extend_from_slice(self.tiles.curves());
        tiles.extend(self.tiles.tiles().iter().map(|tile| PathTile {
            paint: paint_index,
            curve_start: curve_start + tile.curve_start,
            ..*tile
        }));
    }
}

impl From<PaintedPath> for Primitive {
    fn from(path: PaintedPath) -> Self {
        Primitive::Path(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{point, size};

    fn square(x: f32) -> Bounds<ScaledPixels> {
        Bounds {
            origin: point(ScaledPixels(x), ScaledPixels(0.)),
            size: size(ScaledPixels(10.), ScaledPixels(10.)),
        }
    }

    fn quad(x: f32) -> Quad {
        Quad {
            bounds: square(x),
            content_mask: ContentMask { bounds: square(x) },
            ..Default::default()
        }
    }

    fn underline(x: f32) -> Underline {
        Underline {
            order: 0,
            pad: 0,
            bounds: square(x),
            content_mask: ContentMask { bounds: square(x) },
            color: Hsla::default(),
            thickness: ScaledPixels(1.),
            wavy: false.into(),
        }
    }

    fn batch_labels(scene: &mut Scene) -> Vec<String> {
        scene.finish();
        scene.batches().map(|batch| batch.label()).collect()
    }

    #[test]
    fn layers_are_drawn_in_paint_order() {
        // A quad painted after a layer goes above it.
        let mut scene = Scene::default();
        scene.insert_primitive(quad(0.));
        scene.push_layer();
        scene.insert_primitive(underline(0.));
        scene.insert_primitive(underline(5.));
        scene.pop_layer();
        scene.insert_primitive(quad(5.));
        assert_eq!(
            batch_labels(&mut scene),
            ["quads (1)", "underlines (2)", "quads (1)"]
        );

        // A layer goes above the layer painted before it.
        let mut scene = Scene::default();
        scene.push_layer();
        scene.insert_primitive(underline(0.));
        scene.pop_layer();
        scene.push_layer();
        scene.insert_primitive(quad(5.));
        scene.pop_layer();
        assert_eq!(batch_labels(&mut scene), ["underlines (1)", "quads (1)"]);
    }

    #[test]
    fn nested_layers_are_drawn_above_their_parents() {
        // A layer has one draw order, so what it paints after a nested layer
        // still goes below that layer.
        let mut scene = Scene::default();
        scene.push_layer();
        scene.insert_primitive(underline(0.));
        scene.push_layer();
        scene.insert_primitive(quad(5.));
        scene.pop_layer();
        scene.insert_primitive(underline(5.));
        scene.pop_layer();
        assert_eq!(batch_labels(&mut scene), ["underlines (2)", "quads (1)"]);

        // The same holds when the nested layer shares its parent's order.
        let mut scene = Scene::default();
        scene.push_layer();
        scene.insert_primitive(quad(0.));
        scene.push_layer();
        scene.insert_primitive(quad(5.));
        scene.pop_layer();
        scene.insert_primitive(quad(8.));
        scene.pop_layer();
        assert_eq!(batch_labels(&mut scene), ["quads (3)"]);
        assert_eq!(
            scene
                .quads
                .iter()
                .map(|quad| quad.bounds)
                .collect::<Vec<_>>(),
            [square(0.), square(8.), square(5.)]
        );

        // A layer goes above the layers nested in the layer before it.
        let mut scene = Scene::default();
        scene.push_layer();
        scene.push_layer();
        scene.insert_primitive(underline(0.));
        scene.pop_layer();
        scene.pop_layer();
        scene.push_layer();
        scene.insert_primitive(quad(5.));
        scene.pop_layer();
        assert_eq!(batch_labels(&mut scene), ["underlines (1)", "quads (1)"]);
    }
}
