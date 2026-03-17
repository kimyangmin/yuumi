#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinFunction {
    Print,
    Println,
    Range,
}

impl BuiltinFunction {
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "print" => Some(Self::Print),
            "println" => Some(Self::Println),
            "range" => Some(Self::Range),
            _ => None,
        }
    }
}

pub fn is_range_function(name: &str) -> bool {
    matches!(BuiltinFunction::from_name(name), Some(BuiltinFunction::Range))
}

