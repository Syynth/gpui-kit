use std::{ops::Range, rc::Rc};

use gpui::{
    AnyElement, App, AppContext as _, AvailableSpace, Bounds, Element, ElementId, Entity, Global,
    InteractiveElement, IntoElement, MouseDownEvent, MouseMoveEvent, ParentElement as _, Pixels,
    Render, StatefulInteractiveElement as _, StyleRefinement, Styled, Window, deferred, div, point,
    px,
};

use crate::{
    StyledExt, ThemeStyled as _,
    input::{EditorState, popovers::render_markdown},
};

/// What an app-drawn hover card shows: the editor it belongs to (so an
/// action in the card can edit it), the hover the provider answered if any,
/// and every diagnostic covering the pointer.
#[derive(Clone)]
pub struct HoverCard {
    pub editor: Entity<EditorState>,
    pub hover: Option<Rc<lsp_types::Hover>>,
    pub diagnostics: Rc<[crate::highlighter::DiagnosticEntry]>,
}

/// An app's own drawing of hover content, in place of the kit's generic
/// markdown view, and how it wants the card styled. Installed once, for
/// every editor in the app, with [`set_hover_renderer`].
///
/// A language's hover is rarely generic markdown: a kind, a signature, a
/// "defined in" link the app knows how to follow. The kit cannot know that
/// shape, so it hands the hover to the app and keeps only the placement,
/// the dismissal and the card's frame.
#[derive(Clone)]
pub struct HoverRenderer {
    /// Draws the card's content: the hover, and the diagnostics under the
    /// pointer, as one card.
    pub render: Rc<dyn Fn(&HoverCard, &mut Window, &mut App) -> AnyElement>,
    /// Refines the card itself — padding, radius, shadow — over the kit's
    /// own popover chrome.
    pub card: StyleRefinement,
}

impl Global for HoverRenderer {}

/// Draw every editor's hover content with `renderer` from now on.
pub fn set_hover_renderer(renderer: HoverRenderer, cx: &mut App) {
    cx.set_global(renderer);
}

pub struct HoverPopover {
    editor: Entity<EditorState>,
    /// The symbol range byte of the hover trigger.
    pub(crate) symbol_range: Range<usize>,
    pub(crate) hover: Option<Rc<lsp_types::Hover>>,
    /// For an app-drawn card: the diagnostics under the pointer.
    diagnostics: Rc<[crate::highlighter::DiagnosticEntry]>,
}

impl HoverPopover {
    pub fn new(
        editor: Entity<EditorState>,
        symbol_range: Range<usize>,
        hover: &lsp_types::Hover,
        cx: &mut App,
    ) -> Entity<Self> {
        let hover = Rc::new(hover.clone());

        cx.new(|_| Self {
            editor,
            symbol_range,
            hover: Some(hover),
            diagnostics: Rc::from([]),
        })
    }

    /// An app-drawn card: the hover if there is one, and the diagnostics.
    /// Anchored on the hovered symbol, or on the first diagnostic's range
    /// when there is no hover.
    pub(crate) fn card(
        editor: Entity<EditorState>,
        hover: Option<(Range<usize>, lsp_types::Hover)>,
        diagnostics: Rc<[crate::highlighter::DiagnosticEntry]>,
        cx: &mut App,
    ) -> Entity<Self> {
        let (symbol_range, hover) = match hover {
            Some((range, hover)) => (range, Some(Rc::new(hover))),
            None => (
                diagnostics
                    .first()
                    .map(|d| d.range.clone())
                    .unwrap_or_default(),
                None,
            ),
        };
        cx.new(|_| Self {
            editor,
            symbol_range,
            hover,
            diagnostics,
        })
    }
}

impl Render for HoverPopover {
    fn render(&mut self, _: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        if let Some(renderer) = cx.try_global::<HoverRenderer>().cloned() {
            let card = HoverCard {
                editor: self.editor.clone(),
                hover: self.hover.clone(),
                diagnostics: self.diagnostics.clone(),
            };
            let mut popover = Popover::new(
                "hover-popover",
                self.editor.clone(),
                self.symbol_range.clone(),
                move |window, cx| (renderer.render)(&card, window, cx),
            );
            *popover.style() = renderer.card;
            return popover.into_any_element();
        }
        let Some(hover) = self.hover.clone() else {
            return gpui::Empty.into_any_element();
        };
        let contents = match hover.contents.clone() {
            lsp_types::HoverContents::Scalar(scalar) => match scalar {
                lsp_types::MarkedString::String(s) => s,
                lsp_types::MarkedString::LanguageString(ls) => ls.value,
            },
            lsp_types::HoverContents::Array(arr) => arr
                .into_iter()
                .map(|item| match item {
                    lsp_types::MarkedString::String(s) => s,
                    lsp_types::MarkedString::LanguageString(ls) => ls.value,
                })
                .collect::<Vec<_>>()
                .join("\n\n"),
            lsp_types::HoverContents::Markup(markup) => markup.value,
        };

        Popover::new(
            "hover-popover",
            self.editor.clone(),
            self.symbol_range.clone(),
            move |window, cx| render_markdown("message", contents.clone(), window, cx),
        )
        .into_any_element()
    }
}

pub(crate) struct Popover {
    id: ElementId,
    style: StyleRefinement,
    editor: Entity<EditorState>,
    range: Range<usize>,
    width_limit: Range<Pixels>,
    content_builder: Box<dyn Fn(&mut Window, &mut App) -> AnyElement>,
}

impl Styled for Popover {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl Popover {
    pub fn new<F, E>(
        id: impl Into<ElementId>,
        editor: Entity<EditorState>,
        range: Range<usize>,
        f: F,
    ) -> Self
    where
        F: Fn(&mut Window, &mut App) -> E + 'static,
        E: IntoElement,
    {
        Self {
            id: id.into(),
            editor,
            range,
            style: StyleRefinement::default(),
            width_limit: px(200.)..px(500.),
            content_builder: Box::new(move |window, cx| (f)(window, cx).into_any_element()),
        }
    }

    /// Get the bounds of the range in the editor, if it is visible.
    fn trigger_bounds(&self, cx: &App) -> Option<Bounds<Pixels>> {
        let editor = self.editor.read(cx);
        editor.range_to_bounds(&self.range)
    }
}

impl IntoElement for Popover {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

pub(crate) struct PopoverLayoutState {
    bounds: Bounds<Pixels>,
    element: Option<AnyElement>,
}

impl Element for Popover {
    type RequestLayoutState = PopoverLayoutState;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&gpui::GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (gpui::LayoutId, Self::RequestLayoutState) {
        let trigger_bounds = match self.trigger_bounds(cx) {
            Some(bounds) => bounds,
            None => {
                return (
                    div().into_any_element().request_layout(window, cx),
                    PopoverLayoutState {
                        bounds: Bounds::default(),
                        element: None,
                    },
                );
            }
        };

        let max_width = self
            .width_limit
            .end
            .min(window.bounds().size.width - SNAP_TO_EDGE * 2)
            .max(px(200.));
        let max_height = (window.bounds().size.height - SNAP_TO_EDGE * 2).min(px(320.));

        let mut popover = deferred(
            div()
                .id("hover-popover-content")
                .flex_none()
                .occlude()
                .p_1()
                .text_xs()
                .popover_style(cx)
                .shadow_md()
                .max_w(max_width)
                .max_h(max_height)
                .overflow_y_scroll()
                .refine_style(&self.style)
                .child((self.content_builder)(window, cx)),
        )
        .into_any_element();

        let popover_size = popover.layout_as_root(AvailableSpace::min_size(), window, cx);
        const SNAP_TO_EDGE: Pixels = px(8.);
        let top_space = trigger_bounds.top() - SNAP_TO_EDGE;
        let right_space = window.bounds().size.width - trigger_bounds.left() - SNAP_TO_EDGE;

        let mut pos = point(
            trigger_bounds.left(),
            trigger_bounds.top() - popover_size.height,
        );
        if popover_size.height > top_space {
            pos.y = trigger_bounds.bottom();
        }
        if popover_size.width > right_space {
            pos.x = trigger_bounds.right() - popover_size.width;
        }

        let mut empty = div().into_any_element();
        let layout_id = empty.request_layout(window, cx);
        (
            layout_id,
            PopoverLayoutState {
                bounds: Bounds {
                    origin: pos,
                    size: popover_size,
                },
                element: Some(popover),
            },
        )
    }

    fn prepaint(
        &mut self,
        _: Option<&gpui::GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        _: Bounds<Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let bounds = request_layout.bounds;
        let Some(popover) = request_layout.element.as_mut() else {
            return;
        };

        window.with_absolute_element_offset(bounds.origin, |window| {
            popover.prepaint(window, cx);
        })
    }

    fn paint(
        &mut self,
        _: Option<&gpui::GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        _: Bounds<Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        _: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let bounds = request_layout.bounds;
        let Some(popover) = request_layout.element.as_mut() else {
            return;
        };

        popover.paint(window, cx);

        let editor = self.editor.clone();
        // Mouse down out to hide.
        window.on_mouse_event(move |event: &MouseDownEvent, _, _, cx| {
            if !bounds.contains(&event.position) {
                let _ = editor.update(cx, |editor, cx| {
                    editor.clear_hover_state(cx);
                });
            }
        });

        // Mouse out of trigger + popover bounds
        let editor = self.editor.clone();
        let trigger_bounds = self.trigger_bounds(cx).unwrap_or(bounds);
        let keep_open_region = trigger_bounds.union(&bounds);
        window.on_mouse_event(move |event: &MouseMoveEvent, _, _, cx| {
            if !keep_open_region.contains(&event.position) {
                let _ = editor.update(cx, |editor, cx| {
                    editor.clear_hover_state(cx);
                });
            }
        })
    }
}
