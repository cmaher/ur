use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io::Write;
use std::path::Path;

/// oklch(L% C H) -> oklab(L, a, b)
/// L is in [0,1] (input is percentage / 100), C is chroma, H is hue in degrees.
fn oklch_to_oklab(l: f64, c: f64, h_deg: f64) -> (f64, f64, f64) {
    let h_rad = h_deg.to_radians();
    let a = c * h_rad.cos();
    let b = c * h_rad.sin();
    (l, a, b)
}

/// oklab -> linear sRGB using the standard matrix.
/// Reference: https://bottosson.github.io/posts/oklab/
fn oklab_to_linear_srgb(l: f64, a: f64, b: f64) -> (f64, f64, f64) {
    // oklab -> LMS (cube roots)
    let l_ = l + 0.3963377774 * a + 0.2158037573 * b;
    let m_ = l - 0.1055613458 * a - 0.0638541728 * b;
    let s_ = l - 0.0894841775 * a - 1.2914855480 * b;

    let l_cubed = l_ * l_ * l_;
    let m_cubed = m_ * m_ * m_;
    let s_cubed = s_ * s_ * s_;

    // LMS -> linear sRGB
    let r = 4.0767416621 * l_cubed - 3.3077115913 * m_cubed + 0.2309699292 * s_cubed;
    let g = -1.2684380046 * l_cubed + 2.6097574011 * m_cubed - 0.3413193965 * s_cubed;
    let bl = -0.0041960863 * l_cubed - 0.7034186147 * m_cubed + 1.7076147010 * s_cubed;

    (r, g, bl)
}

/// linear sRGB component -> sRGB gamma-corrected component.
fn linear_to_srgb_gamma(c: f64) -> f64 {
    if c <= 0.0031308 {
        12.92 * c
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// Convert oklch(L%, C, H) to sRGB (r, g, b) each in [0, 255].
fn oklch_to_srgb(l_pct: f64, c: f64, h_deg: f64) -> (u8, u8, u8) {
    let l = l_pct / 100.0;
    let (ol, oa, ob) = oklch_to_oklab(l, c, h_deg);
    let (lr, lg, lb) = oklab_to_linear_srgb(ol, oa, ob);

    let r = linear_to_srgb_gamma(lr);
    let g = linear_to_srgb_gamma(lg);
    let b = linear_to_srgb_gamma(lb);

    let clamp = |v: f64| -> u8 { (v * 255.0).round().clamp(0.0, 255.0) as u8 };

    (clamp(r), clamp(g), clamp(b))
}

/// CSS variable short name -> Theme field name mapping.
fn css_var_to_field(var: &str) -> Option<&'static str> {
    match var {
        "p" => Some("primary"),
        "s" => Some("secondary"),
        "a" => Some("accent"),
        "n" => Some("neutral"),
        "b1" => Some("base_100"),
        "b2" => Some("base_200"),
        "b3" => Some("base_300"),
        "bc" => Some("base_content"),
        "pc" => Some("primary_content"),
        "sc" => Some("secondary_content"),
        "ac" => Some("accent_content"),
        "nc" => Some("neutral_content"),
        "in" => Some("info"),
        "su" => Some("success"),
        "wa" => Some("warning"),
        "er" => Some("error"),
        "inc" => Some("info_content"),
        "suc" => Some("success_content"),
        "wac" => Some("warning_content"),
        "erc" => Some("error_content"),
        _ => None,
    }
}

/// Parse an oklch value string like "oklch(65.69% 0.196 275.75)" into (L%, C, H).
fn parse_oklch(value: &str) -> Option<(f64, f64, f64)> {
    let inner = value
        .trim()
        .strip_prefix("oklch(")?
        .strip_suffix(')')?
        .trim();
    let parts: Vec<&str> = inner.split_whitespace().collect();
    if parts.len() != 3 {
        return None;
    }
    let l_pct: f64 = parts[0].strip_suffix('%')?.parse().ok()?;
    let c: f64 = parts[1].parse().ok()?;
    let h: f64 = parts[2].parse().ok()?;
    Some((l_pct, c, h))
}

/// All expected theme field names, in order.
const THEME_FIELDS: &[&str] = &[
    "primary",
    "secondary",
    "accent",
    "neutral",
    "base_100",
    "base_200",
    "base_300",
    "base_content",
    "primary_content",
    "secondary_content",
    "accent_content",
    "neutral_content",
    "info",
    "success",
    "warning",
    "error",
    "info_content",
    "success_content",
    "warning_content",
    "error_content",
];

fn main() {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let themes_css_path = Path::new(&manifest_dir).join("themes").join("themes.css");

    println!(
        "cargo:rerun-if-changed={}",
        themes_css_path.to_string_lossy()
    );

    let css = fs::read_to_string(&themes_css_path)
        .unwrap_or_else(|e| panic!("Failed to read {}: {}", themes_css_path.display(), e));

    // Parse themes: theme_name -> { field_name -> (r, g, b) }
    let themes = parse_themes(&css);

    // Generate Rust source
    let out_dir = env::var("OUT_DIR").unwrap();
    let out_path = Path::new(&out_dir).join("builtin_themes.rs");
    let mut out = fs::File::create(&out_path).unwrap();

    write_generated_code(&mut out, &themes);
}

/// Parse a single CSS variable line and insert the converted color into the themes map.
fn parse_css_var(
    rest: &str,
    colon_pos: usize,
    theme_name: &str,
    themes: &mut BTreeMap<String, BTreeMap<String, (u8, u8, u8)>>,
) {
    let var_name = rest[..colon_pos].trim();
    let value = rest[colon_pos + 1..].trim().trim_end_matches(';').trim();

    let Some(field_name) = css_var_to_field(var_name) else {
        return;
    };
    let Some((l, c, h)) = parse_oklch(value) else {
        panic!(
            "Failed to parse oklch value for --{} in theme '{}': {}",
            var_name, theme_name, value
        );
    };
    let (r, g, b) = oklch_to_srgb(l, c, h);
    themes
        .entry(theme_name.to_string())
        .or_default()
        .insert(field_name.to_string(), (r, g, b));
}

/// Parse all themes from CSS content.
fn parse_themes(css: &str) -> BTreeMap<String, BTreeMap<String, (u8, u8, u8)>> {
    let mut themes: BTreeMap<String, BTreeMap<String, (u8, u8, u8)>> = BTreeMap::new();
    let mut current_theme: Option<String> = None;

    for line in css.lines() {
        let trimmed = line.trim();

        // Match theme selector: [data-theme="name"] {
        if let Some(rest) = trimmed.strip_prefix("[data-theme=\"")
            && let Some(name_end) = rest.find('"')
        {
            let name = rest[..name_end].to_string();
            current_theme = Some(name);
            continue;
        }

        // Match closing brace
        if trimmed == "}" {
            current_theme = None;
            continue;
        }

        // Match CSS variable: --varname: oklch(...);
        if let Some(theme_name) = &current_theme
            && let Some(rest) = trimmed.strip_prefix("--")
            && let Some(colon_pos) = rest.find(':')
        {
            parse_css_var(rest, colon_pos, theme_name, &mut themes);
        }
    }

    // Validate all themes have all fields
    for (theme_name, fields) in &themes {
        for &expected_field in THEME_FIELDS {
            if !fields.contains_key(expected_field) {
                panic!(
                    "Theme '{}' is missing field '{}'",
                    theme_name, expected_field
                );
            }
        }
    }

    themes
}

fn write_generated_code(
    out: &mut fs::File,
    themes: &BTreeMap<String, BTreeMap<String, (u8, u8, u8)>>,
) {
    writeln!(out, "// Generated by build.rs from themes/themes.css").unwrap();
    writeln!(out, "// Do not edit manually.").unwrap();
    writeln!(out).unwrap();
    writeln!(
        out,
        "/// Returns a built-in daisyUI theme by name, if it exists."
    )
    .unwrap();
    writeln!(out, "pub fn builtin_theme(name: &str) -> Option<Theme> {{").unwrap();
    writeln!(out, "    match name {{").unwrap();

    for (theme_name, fields) in themes {
        writeln!(out, "        \"{}\" => Some(Theme {{", theme_name).unwrap();
        for &field in THEME_FIELDS {
            let (r, g, b) = fields[field];
            writeln!(
                out,
                "            {}: Color::Rgb({}, {}, {}),",
                field, r, g, b
            )
            .unwrap();
        }
        writeln!(out, "        }}),").unwrap();
    }

    writeln!(out, "        _ => None,").unwrap();
    writeln!(out, "    }}").unwrap();
    writeln!(out, "}}").unwrap();
    writeln!(out).unwrap();

    // Also generate a list of all theme names
    writeln!(out, "/// All built-in theme names, sorted alphabetically.").unwrap();
    writeln!(out, "pub const BUILTIN_THEME_NAMES: &[&str] = &[").unwrap();
    for theme_name in themes.keys() {
        writeln!(out, "    \"{}\",", theme_name).unwrap();
    }
    writeln!(out, "];").unwrap();
}
