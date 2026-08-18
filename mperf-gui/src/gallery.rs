use gpui::{
    Axis, Context, Entity, FontWeight, IntoElement, Render, SharedString, Subscription, Window,
    WindowAppearance, div, prelude::*, px, uniform_list,
};

use crate::ui::{
    self, ActiveTheme, BadgeVariant, ButtonSize, ButtonVariant, Column, DropdownItem, Icon,
    TabItem, Theme, badge, button, card, card_description, card_section, card_title, chip,
    collapsible_section, dialog, dropdown_menu, empty_state, info_tooltip, meter, section_caption,
    segment_bar, segmented, separator, stat_tile, tab_bar, table_cell, table_header, table_row,
};

/// `--gallery`: every widget with fake data, in both themes — the visual
/// parity checklist against the React prototype.
pub struct Gallery {
    override_dark: Option<bool>,
    active_tab: usize,
    segment_ix: usize,
    dropdown_open: bool,
    checks: [bool; 3],
    dialog_open: bool,
    section_open: bool,
    selected_row: Option<usize>,
    sort_descending: bool,
    search_input: Entity<ui::TextInput>,
    plain_input: Entity<ui::TextInput>,
    splitter: Entity<ui::Splitter>,
    panel_width: f32,
    _appearance: Subscription,
}

const THREADS: [&str; 3] = ["physim", "omp worker 1", "omp worker 7"];

impl Gallery {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search_input = cx.new(|cx| {
            ui::TextInput::new("gallery-search", "match symbol…", cx)
                .leading_icon(Icon::Search)
                .clearable()
                .kbd_hint("⌘F")
                .height(24.0)
                .text_size(11.0)
                .width(176.0)
        });
        let plain_input =
            cx.new(|cx| ui::TextInput::new("gallery-plain", "Type here…", cx).width(220.0));
        let entity = cx.entity();
        let splitter = cx.new(|_| {
            ui::Splitter::new(Axis::Horizontal, move |position, _, cx| {
                entity.update(cx, |this, cx| {
                    this.panel_width = (f32::from(position) - 25.0).clamp(120.0, 480.0);
                    cx.notify();
                });
            })
        });
        let appearance = cx.observe_window_appearance(window, |this: &mut Self, window, cx| {
            if this.override_dark.is_none() {
                cx.set_global(Theme::from_appearance(window.appearance()));
            }
            cx.notify();
        });
        Self {
            override_dark: None,
            active_tab: 1,
            segment_ix: 0,
            dropdown_open: false,
            checks: [true, true, false],
            dialog_open: false,
            section_open: true,
            selected_row: Some(0),
            sort_descending: true,
            search_input,
            plain_input,
            splitter,
            panel_width: 260.0,
            _appearance: appearance,
        }
    }

    fn toggle_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let dark = match self.override_dark {
            Some(dark) => !dark,
            None => !matches!(
                window.appearance(),
                WindowAppearance::Dark | WindowAppearance::VibrantDark
            ),
        };
        self.override_dark = Some(dark);
        cx.set_global(if dark { Theme::dark() } else { Theme::light() });
        cx.notify();
    }

    fn section(&self, title: &'static str, cx: &Context<Self>) -> gpui::Div {
        div().flex().flex_col().gap(px(8.0)).child(
            div()
                .text_size(px(12.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(cx.theme().muted_foreground)
                .child(title.to_uppercase()),
        )
    }
}

impl Render for Gallery {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();

        let hotspot_rows: [(&str, &str, &str, f32); 5] = [
            ("physim", "gather_neighbors", "22.2%", 0.53),
            ("physim", "cell_interactions", "15.7%", 0.38),
            ("physim", "hash_lookup", "11.4%", 0.27),
            ("[kernel]", "futex_wait", "9.4%", 0.23),
            ("libm", "expf", "5.3%", 0.13),
        ];
        let columns = vec![
            Column::new("Function"),
            Column::new("Self %").width(80.0).right().sortable(),
            Column::new("Total %").width(80.0).right(),
        ];

        let buttons = self
            .section("Button", cx)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(button("b-default").label("Default"))
                    .child(
                        button("b-outline")
                            .label("Outline")
                            .variant(ButtonVariant::Outline),
                    )
                    .child(
                        button("b-secondary")
                            .label("Secondary")
                            .variant(ButtonVariant::Secondary),
                    )
                    .child(
                        button("b-ghost")
                            .label("Ghost")
                            .variant(ButtonVariant::Ghost),
                    )
                    .child(
                        button("b-destructive")
                            .label("Destructive")
                            .variant(ButtonVariant::Destructive),
                    )
                    .child(button("b-link").label("Link").variant(ButtonVariant::Link))
                    .child(button("b-disabled").label("Disabled").disabled(true)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(button("b-lg").label("Large").size(ButtonSize::Lg))
                    .child(button("b-sm").label("Small").size(ButtonSize::Sm))
                    .child(button("b-xs").label("Extra small").size(ButtonSize::Xs))
                    .child(
                        button("b-icon-new")
                            .icon(Icon::CircleDot)
                            .label("New profile")
                            .variant(ButtonVariant::Outline)
                            .size(ButtonSize::Xs),
                    )
                    .child(
                        button("b-icon")
                            .icon(Icon::Play)
                            .variant(ButtonVariant::Outline)
                            .size(ButtonSize::Icon),
                    )
                    .child(
                        button("b-icon-sm")
                            .icon(Icon::PanelRightClose)
                            .variant(ButtonVariant::Ghost)
                            .size(ButtonSize::IconSm),
                    )
                    .child(
                        button("b-icon-xs")
                            .icon(Icon::X)
                            .variant(ButtonVariant::Ghost)
                            .size(ButtonSize::IconXs),
                    ),
            );

        let badges = self.section("Badge · Chip", cx).child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(badge("Badge"))
                .child(badge("Secondary").variant(BadgeVariant::Secondary))
                .child(badge("Destructive").variant(BadgeVariant::Destructive))
                .child(badge("Outline").variant(BadgeVariant::Outline))
                .child(badge("Ghost").variant(BadgeVariant::Ghost))
                .child(badge("physim").tint(theme.viz.series[0]))
                .child(badge("libc").tint(theme.viz.series[2]))
                .child(badge("[kernel]").tint(theme.viz.series[1]))
                .child(chip("chip-full", "full run").icon(Icon::Clock))
                .child(
                    chip("chip-range", "3.90s – 6.40s")
                        .icon(Icon::Clock)
                        .active(true)
                        .on_close(|_, _| {}),
                )
                .child(
                    chip("chip-frame", "gather_neighbors")
                        .mono(true)
                        .active(true)
                        .on_close(|_, _| {}),
                ),
        );

        let tabs = self
            .section("Tabs · Segmented", cx)
            .child(
                tab_bar(
                    "gallery-tabs",
                    vec![
                        TabItem::new("Summary").icon(Icon::LayoutDashboard),
                        TabItem::new("Hotspots").icon(Icon::Table2),
                        TabItem::new("Flame Graph").icon(Icon::Flame),
                        TabItem::new("Top-Down").icon(Icon::Layers),
                        TabItem::new("physim.c")
                            .icon(Icon::FileCode2)
                            .mono()
                            .closable(),
                    ],
                    self.active_tab,
                )
                .on_select(cx.processor(|this, ix: usize, _, cx| {
                    this.active_tab = ix;
                    cx.notify();
                }))
                .on_close(cx.processor(|_, _: usize, _, cx| cx.notify())),
            )
            .child(
                segmented(
                    "gallery-segmented",
                    vec!["top-down".into(), "bottom-up".into(), "flat".into()],
                    self.segment_ix,
                )
                .on_select(cx.processor(|this, ix: usize, _, cx| {
                    this.segment_ix = ix;
                    cx.notify();
                })),
            );

        let dropdown_items: Vec<DropdownItem> = THREADS
            .iter()
            .enumerate()
            .map(|(ix, name)| {
                DropdownItem::new(*name)
                    .checked(self.checks[ix])
                    .trailing(["41200", "41202", "41208"][ix])
            })
            .chain([DropdownItem::new("All threads").separator_before()])
            .collect();
        let checked_count = self.checks.iter().filter(|c| **c).count();

        let inputs = self.section("Input · Dropdown · Tooltip", cx).child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(self.plain_input.clone())
                .child(self.search_input.clone())
                .child(
                    dropdown_menu(
                        "gallery-dropdown",
                        button("dd-trigger")
                            .label(if checked_count == THREADS.len() {
                                "all threads".to_string()
                            } else {
                                format!("{checked_count} threads")
                            })
                            .variant(ButtonVariant::Outline)
                            .size(ButtonSize::Xs)
                            .toggled(self.dropdown_open),
                        self.dropdown_open,
                    )
                    .min_width(208.0)
                    .items(dropdown_items)
                    .on_toggle(cx.processor(|this, open: bool, _, cx| {
                        this.dropdown_open = open;
                        cx.notify();
                    }))
                    .on_select(cx.processor(|this, ix: usize, _, cx| {
                        if ix < this.checks.len() {
                            this.checks[ix] = !this.checks[ix];
                        } else {
                            this.checks = [true; 3];
                        }
                        cx.notify();
                    })),
                )
                .child(
                    button("open-dialog")
                        .label("Open dialog")
                        .variant(ButtonVariant::Outline)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.dialog_open = true;
                            cx.notify();
                        })),
                )
                .child(info_tooltip(
                    "gallery-info",
                    "Fraction of pipeline slots wasted on mispredicted branches.",
                    cx,
                )),
        );

        let kbds = self.section("Kbd", cx).child(
            div()
                .flex()
                .items_center()
                .gap(px(12.0))
                .child(ui::kbd("⌘F"))
                .child(ui::kbd("esc"))
                .child(ui::kbd_group(["⌘", "shift", "P"]))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .text_size(px(11.0))
                        .text_color(theme.muted_foreground)
                        .child("use")
                        .child(ui::kbd("⌘K"))
                        .child("to open the command palette"),
                ),
        );

        let meters = self
            .section("Meter · Segment bar · Stat tiles", cx)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .w(px(300.0))
                    .child(meter(0.79))
                    .child(meter(0.46).color(theme.viz.status_warn))
                    .child(meter(0.92).color(theme.viz.status_critical))
                    .child(segment_bar(vec![
                        (0.18, theme.viz.series[2]),
                        (0.05, theme.viz.series[4]),
                        (0.06, theme.viz.series[3]),
                        (0.71, theme.viz.series[1]),
                    ])),
            )
            .child(
                div()
                    .flex()
                    .gap(px(4.0))
                    .w(px(300.0))
                    .child(stat_tile("IPC", "0.78"))
                    .child(stat_tile("LLC MPKI", "47.1"))
                    .child(stat_tile("BE stall", "82%")),
            );

        let sort = self.sort_descending;
        let selected_row = self.selected_row;
        let entity = cx.entity();
        let row_data: Vec<(SharedString, SharedString, SharedString, f32)> = hotspot_rows
            .iter()
            .map(|(module, name, pct, bar)| {
                (
                    SharedString::from(*module),
                    SharedString::from(*name),
                    SharedString::from(*pct),
                    *bar,
                )
            })
            .collect();
        let table_columns = columns.clone();

        let table = self.section("Virtual table", cx).child(
            div()
                .flex()
                .flex_col()
                .h(px(180.0))
                .w(px(460.0))
                .border_1()
                .border_color(theme.border)
                .rounded(theme.radius_md())
                .overflow_hidden()
                .child(table_header(&columns, Some((1, sort)), cx))
                .child(
                    uniform_list("gallery-table", row_data.len(), move |range, _, cx| {
                        let theme = cx.theme().clone();
                        range
                            .map(|ix| {
                                let (module, name, pct, bar) = row_data[ix].clone();
                                let entity = entity.clone();
                                let module_color = match module.as_ref() {
                                    "[kernel]" => theme.viz.series[1],
                                    "libm" | "libc" => theme.viz.series[2],
                                    _ => theme.viz.series[0],
                                };
                                div()
                                    .id(ix)
                                    .w_full()
                                    .child(
                                        table_row(selected_row == Some(ix), cx)
                                            .child(
                                                table_cell(&table_columns[0])
                                                    .gap(px(6.0))
                                                    .child(badge(module).tint(module_color))
                                                    .child(
                                                        div()
                                                            .font_family(theme.font_mono.clone())
                                                            .text_size(px(11.0))
                                                            .truncate()
                                                            .child(name),
                                                    ),
                                            )
                                            .child(
                                                table_cell(&table_columns[1])
                                                    .relative()
                                                    .child(
                                                        div()
                                                            .absolute()
                                                            .left_0()
                                                            .top(px(4.0))
                                                            .bottom(px(4.0))
                                                            .w(px(76.0 * bar))
                                                            .rounded_r(px(2.0))
                                                            .bg(theme.viz.series[0].opacity(0.18)),
                                                    )
                                                    .child(
                                                        div()
                                                            .relative()
                                                            .text_size(px(11.0))
                                                            .child(pct.clone()),
                                                    ),
                                            )
                                            .child(
                                                table_cell(&table_columns[2]).child(
                                                    div()
                                                        .text_size(px(11.0))
                                                        .text_color(theme.muted_foreground)
                                                        .child(pct),
                                                ),
                                            ),
                                    )
                                    .on_click(move |_, _, cx| {
                                        entity.update(cx, |this, cx| {
                                            this.selected_row = Some(ix);
                                            cx.notify();
                                        });
                                    })
                            })
                            .collect()
                    })
                    .flex_1()
                    .min_h(px(0.0)),
                ),
        );

        let cards = self.section("Card · Empty state · Collapsible", cx).child(
            div()
                .flex()
                .items_start()
                .gap(px(16.0))
                .child(
                    card(cx)
                        .w(px(280.0))
                        .child(card_title(cx).child("Backend Bound"))
                        .child(
                            card_description(cx)
                                .child("46% of slots · Memory Bound 33% · mostly DRAM"),
                        )
                        .child(card_section().child(meter(0.46).color(theme.viz.series[1]))),
                )
                .child(
                    div()
                        .w(px(280.0))
                        .h(px(120.0))
                        .border_1()
                        .border_color(theme.border)
                        .rounded(theme.radius_md())
                        .child(
                            empty_state(
                                Icon::MemoryStick,
                                "no memory observations in this recording",
                            )
                            .action(
                                button("empty-action")
                                    .label("Record with -s memory")
                                    .variant(ButtonVariant::Outline)
                                    .size(ButtonSize::Xs),
                            ),
                        ),
                )
                .child(
                    div()
                        .w(px(280.0))
                        .border_1()
                        .border_color(theme.border)
                        .rounded(theme.radius_md())
                        .overflow_hidden()
                        .child(
                            collapsible_section(
                                "gallery-section",
                                "Master timeline",
                                self.section_open,
                            )
                            .on_toggle({
                                let entity = cx.entity();
                                move |_, cx| {
                                    entity.update(cx, |this, cx| {
                                        this.section_open = !this.section_open;
                                        cx.notify();
                                    })
                                }
                            })
                            .child(
                                div()
                                    .p(px(8.0))
                                    .text_size(px(11.0))
                                    .text_color(theme.muted_foreground)
                                    .child("collapsible content"),
                            ),
                        ),
                ),
        );

        let splitter_demo = self.section("Splitter", cx).child(
            div()
                .flex()
                .h(px(80.0))
                .w(px(460.0))
                .border_1()
                .border_color(theme.border)
                .rounded(theme.radius_md())
                .overflow_hidden()
                .child(
                    div()
                        .w(px(self.panel_width))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(11.0))
                        .text_color(theme.muted_foreground)
                        .child(format!("{}px", self.panel_width as i32)),
                )
                .child(self.splitter.clone())
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(11.0))
                        .text_color(theme.muted_foreground)
                        .child("drag the divider"),
                ),
        );

        div()
            .id("gallery-root")
            .size_full()
            .overflow_y_scroll()
            .bg(theme.background)
            .text_color(theme.foreground)
            .font_family(theme.font_ui.clone())
            .text_size(px(13.0))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(24.0))
                    .p(px(24.0))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_size(px(16.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("mperf ui gallery"),
                            )
                            .child(
                                button("theme-toggle")
                                    .icon(if theme.dark { Icon::Sun } else { Icon::Moon })
                                    .variant(ButtonVariant::Ghost)
                                    .size(ButtonSize::IconSm)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.toggle_theme(window, cx)
                                    })),
                            ),
                    )
                    .child(buttons)
                    .child(separator(cx))
                    .child(badges)
                    .child(separator(cx))
                    .child(tabs)
                    .child(separator(cx))
                    .child(inputs)
                    .child(separator(cx))
                    .child(kbds)
                    .child(separator(cx))
                    .child(meters)
                    .child(separator(cx))
                    .child(table)
                    .child(separator(cx))
                    .child(cards)
                    .child(separator(cx))
                    .child(splitter_demo)
                    .child(div().h(px(40.0)))
                    .child(div().child(section_caption("both themes must pass parity review", cx))),
            )
            .when(self.dialog_open, |el| {
                el.child(
                    dialog("gallery-dialog", "New profile")
                        .description(
                            "Record a new profile locally or on a remote host over ssh. \
                             This dialog is the shell for the three-step runner wizard.",
                        )
                        .on_close({
                            let entity = cx.entity();
                            move |_, cx| {
                                entity.update(cx, |this, cx| {
                                    this.dialog_open = false;
                                    cx.notify();
                                })
                            }
                        })
                        .child(
                            div()
                                .flex()
                                .justify_end()
                                .gap(px(8.0))
                                .child(
                                    button("dialog-cancel")
                                        .label("Cancel")
                                        .variant(ButtonVariant::Outline)
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.dialog_open = false;
                                            cx.notify();
                                        })),
                                )
                                .child(button("dialog-start").label("Start").icon(Icon::Play)),
                        ),
                )
            })
    }
}
