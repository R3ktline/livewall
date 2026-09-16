use serde::{Deserialize, Serialize};

/// How the wallpaper is scaled onto an output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ScaleMode {
    /// Cover the output, cropping as needed.
    #[default]
    Fill,
    /// Fit inside the output, letterboxing if needed.
    Fit,
    /// Stretch to exact output size.
    Stretch,
    /// Center at 1:1 (or nearest), no upscale beyond source if possible.
    Center,
}

impl ScaleMode {
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "fill" | "cover" => Some(Self::Fill),
            "fit" | "contain" => Some(Self::Fit),
            "stretch" => Some(Self::Stretch),
            "center" => Some(Self::Center),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fill => "fill",
            Self::Fit => "fit",
            Self::Stretch => "stretch",
            Self::Center => "center",
        }
    }
}

/// Preferred hardware decode backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum HwDecPreference {
    /// Prefer VA-API, then Vulkan Video, then soft (with warning).
    #[default]
    Auto,
    Vaapi,
    Vulkan,
    /// Software decode only (inefficient; always warned).
    Soft,
}

impl HwDecPreference {
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "auto" => Some(Self::Auto),
            "vaapi" => Some(Self::Vaapi),
            "vulkan" | "vulkan-video" => Some(Self::Vulkan),
            "soft" | "software" | "sw" => Some(Self::Soft),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Vaapi => "vaapi",
            Self::Vulkan => "vulkan",
            Self::Soft => "soft",
        }
    }
}

/// Detected wallpaper media kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WallpaperKind {
    Image,
    AnimatedImage,
    Video,
}

impl WallpaperKind {
    pub fn from_path(path: &std::path::Path) -> Self {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        match ext.as_str() {
            "mp4" | "mkv" | "webm" | "mov" | "avi" | "m4v" | "ts" => Self::Video,
            "gif" | "webp" => Self::AnimatedImage,
            _ => Self::Image,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::AnimatedImage => "animated_image",
            Self::Video => "video",
        }
    }
}
