use std::sync::LazyLock;

use rune::Color;
use rune::color::hex;
use rune_kit::Kit;

use crate::prefs::Accent;

pub struct Theme {
    pub base: Kit,
    pub panels: Kit,
    pub controls: Kit,
}

// Each accent has separate ink for light and dark surfaces; swatch colors alone
// don't have enough contrast for links in both appearances.
const ACCENTS: [(&str, &str, &str); 6] = [
    ("#3b82f6", "#1756a9", "#9dc6ff"),
    ("#a371f7", "#6d28b5", "#d0b3ff"),
    ("#ed4f9b", "#a51b65", "#ffadda"),
    ("#f58a25", "#95420c", "#ffc18a"),
    ("#30b873", "#16603d", "#8fdfb5"),
    ("#888d96", "#4b5563", "#c3c7cf"),
];

static THEMES: LazyLock<[[Theme; 6]; 2]> = LazyLock::new(|| {
    [false, true].map(|dark| {
        ACCENTS.map(|(swatch, light, dark_ink)| {
            let color = |s| hex(s).expect("accent colors are valid hex");
            let mut base = rune_theme_opencode::kit(dark).clone();
            base.c.bg_accent = color(swatch);
            base.c.text_accent = color(if dark { dark_ink } else { light });
            base.c.icon_accent = base.c.text_accent;
            base.c.border_focus = base.c.text_accent;
            let mut panels = base.clone();
            if dark {
                panels.c.bg_deep = base.c.bg_base.mix(base.c.layer_01, 0.5);
            }
            let mut controls = base.clone();
            controls.c.bg_base = base.c.bg_base.mix(base.c.text_base, 0.035);
            Theme {
                base,
                panels,
                controls,
            }
        })
    })
});

pub fn get(dark: bool, accent: Accent) -> &'static Theme {
    &THEMES[usize::from(dark)][accent as usize]
}

pub fn message_bg(k: &Kit) -> Color {
    k.c.layer_03.mix(k.c.bg_accent, 0.12)
}

pub fn inline_code(k: &Kit) -> Color {
    k.c.text_accent.mix(k.c.text_base, 0.3)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accent_text_is_readable_on_chat_surfaces() {
        for dark in [false, true] {
            for accent in Accent::ALL {
                let k = &get(dark, accent).base;
                for bg in [k.c.bg_base, k.c.layer_01, message_bg(k)] {
                    for fg in [k.c.text_base, k.c.text_accent, inline_code(k)] {
                        let a = fg.luminance();
                        let b = bg.luminance();
                        let contrast = (a.max(b) + 0.05) / (a.min(b) + 0.05);
                        assert!(
                            contrast >= 4.5,
                            "{accent:?}, dark={dark}: {} on {} has contrast {contrast}",
                            fg.hex(),
                            bg.hex()
                        );
                    }
                }
            }
        }
    }
}
