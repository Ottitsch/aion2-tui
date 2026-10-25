//! Class table: id to name and colour, the same as the aion2dps overlay (`src/shared/classes.ts`), so a
//! Sorcerer's bar is the same purple in the overlay and in the terminal.

use ratatui::style::Color;

pub struct ClassInfo {
    pub id: u32,
    pub name: &'static str,
    pub color: Color,
}

const fn rgb(hex: u32) -> Color {
    Color::Rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

pub const CLASSES: [ClassInfo; 9] = [
    ClassInfo { id: 11, name: "Gladiator", color: rgb(0xe25c5c) },
    ClassInfo { id: 12, name: "Templar", color: rgb(0x5f8cff) },
    ClassInfo { id: 13, name: "Assassin", color: rgb(0xf06fa6) },
    ClassInfo { id: 14, name: "Ranger", color: rgb(0x41d98a) },
    ClassInfo { id: 15, name: "Sorcerer", color: rgb(0x9a6bff) },
    ClassInfo { id: 16, name: "Elementalist", color: rgb(0x3fc9c0) },
    ClassInfo { id: 17, name: "Cleric", color: rgb(0xf2c15a) },
    ClassInfo { id: 18, name: "Chanter", color: rgb(0xff9a3d) },
    ClassInfo { id: 19, name: "Brawler", color: rgb(0xc8d44a) },
];

pub const UNKNOWN: ClassInfo = ClassInfo { id: 0, name: "Unknown", color: rgb(0x8b94a7) };

/// Looks a class up by id, falling back to the name (history rows only carry the name); never fails.
pub fn class_info(class_id: Option<u32>, class_name: Option<&str>) -> &'static ClassInfo {
    if let Some(c) = class_id.and_then(|id| CLASSES.iter().find(|c| c.id == id)) {
        return c;
    }
    if let Some(c) = class_name.and_then(|n| CLASSES.iter().find(|c| c.name.eq_ignore_ascii_case(n.trim()))) {
        return c;
    }
    &UNKNOWN
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_classes_by_id_then_name() {
        assert_eq!(class_info(Some(15), None).name, "Sorcerer");
        assert_eq!(class_info(None, Some(" cleric ")).id, 17);
        assert_eq!(class_info(Some(99), Some("Brawler")).id, 19);
        assert_eq!(class_info(None, None).id, 0);
    }
}
