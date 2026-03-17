#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinFunction {
    Print,
    Println,
    Input,
    TypeOf,
    StrCast,
    IntCast,
    FloatCast,
    DoubleCast,
    Range,
}

impl BuiltinFunction {
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "print" => Some(Self::Print),
            "println" => Some(Self::Println),
            "input" => Some(Self::Input),
            "type" => Some(Self::TypeOf),
            "str" => Some(Self::StrCast),
            "int" => Some(Self::IntCast),
            "float" => Some(Self::FloatCast),
            "double" => Some(Self::DoubleCast),
            "range" => Some(Self::Range),
            _ => None,
        }
    }
}

pub fn is_range_function(name: &str) -> bool {
    matches!(BuiltinFunction::from_name(name), Some(BuiltinFunction::Range))
}

