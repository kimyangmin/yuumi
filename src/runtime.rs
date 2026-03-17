use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeName {
    Int,
    Float,
    Double,
    Bool,
    Str,
    Named(String),
}

impl TypeName {
    pub fn keyword(&self) -> &str {
        match self {
            Self::Int => "int",
            Self::Float => "float",
            Self::Double => "double",
            Self::Bool => "bool",
            Self::Str => "str",
            Self::Named(name) => name.as_str(),
        }
    }

    pub fn is_numeric(&self) -> bool {
        matches!(self, Self::Int | Self::Float | Self::Double)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindingMode {
    Owned,
    SharedBorrow,
    MutableBorrow,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reference {
    pub slot: usize,
    pub mutable: bool,
    pub ty: TypeName,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BorrowState {
    pub shared_count: usize,
    pub mutable_active: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Int(i64),
    Float(f32),
    Double(f64),
    Bool(bool),
    Str(String),
    Object { class_name: String, object_id: usize },
    Ref(Reference),
    Unit,
}

impl Value {
    pub fn ty(&self) -> Option<TypeName> {
        match self {
            Self::Int(_) => Some(TypeName::Int),
            Self::Float(_) => Some(TypeName::Float),
            Self::Double(_) => Some(TypeName::Double),
            Self::Bool(_) => Some(TypeName::Bool),
            Self::Str(_) => Some(TypeName::Str),
            Self::Object { class_name, .. } => Some(TypeName::Named(class_name.clone())),
            Self::Ref(reference) => Some(reference.ty.clone()),
            Self::Unit => None,
        }
    }

    pub fn truthy(&self) -> bool {
        match self {
            Self::Int(value) => *value != 0,
            Self::Float(value) => *value != 0.0,
            Self::Double(value) => *value != 0.0,
            Self::Bool(value) => *value,
            Self::Str(value) => !value.is_empty(),
            Self::Object { .. } => true,
            Self::Ref(_) => true,
            Self::Unit => false,
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Int(value) => write!(f, "{value}"),
            Self::Float(value) => write!(f, "{value}"),
            Self::Double(value) => write!(f, "{value}"),
            Self::Bool(value) => {
                if *value {
                    write!(f, "True")
                } else {
                    write!(f, "False")
                }
            }
            Self::Str(value) => write!(f, "{value}"),
            Self::Object { class_name, object_id } => write!(f, "<{class_name}#{object_id}>"),
            Self::Ref(reference) => write!(f, "<ref:{}:{}>", if reference.mutable { "mut" } else { "shared" }, reference.ty.keyword()),
            Self::Unit => write!(f, "()"),
        }
    }
}

pub fn convert_value(value: Value, target: TypeName) -> Result<Value, String> {
    match (value, target) {
        (Value::Int(value), TypeName::Int) => Ok(Value::Int(value)),
        (Value::Int(value), TypeName::Float) => Ok(Value::Float(value as f32)),
        (Value::Int(value), TypeName::Double) => Ok(Value::Double(value as f64)),
        (Value::Int(value), TypeName::Bool) => Ok(Value::Bool(value != 0)),
        (Value::Float(value), TypeName::Float) => Ok(Value::Float(value)),
        (Value::Float(value), TypeName::Double) => Ok(Value::Double(value as f64)),
        (Value::Double(value), TypeName::Double) => Ok(Value::Double(value)),
        (Value::Bool(value), TypeName::Bool) => Ok(Value::Bool(value)),
        (Value::Str(value), TypeName::Str) => Ok(Value::Str(value)),
        (Value::Bool(value), TypeName::Int) => Ok(Value::Int(i64::from(value))),
        (Value::Bool(value), TypeName::Float) => Ok(Value::Float(if value { 1.0 } else { 0.0 })),
        (Value::Bool(value), TypeName::Double) => Ok(Value::Double(if value { 1.0 } else { 0.0 })),
        (Value::Object { class_name, object_id }, TypeName::Named(expected)) if class_name == expected => {
            Ok(Value::Object { class_name, object_id })
        }
        (Value::Unit, _) => Err("unit value cannot be assigned".to_string()),
        (Value::Ref(reference), _) => Err(format!("reference to '{}' cannot be assigned without dereference", reference.ty.keyword())),
        (source, target) => Err(format!(
            "cannot assign value of type '{}' to '{}'",
            source.ty().as_ref().map(|ty| ty.keyword()).unwrap_or("unit"),
            target.keyword()
        )),
    }
}

