//! Purpose: emit concrete serde types for every Protocol schema node without runtime Protocol coupling.
//! Scope: deterministic build-time type lowering.
//! Uses:
//! `-- rust/xtask/src/codegen/mod.rs
//! External:
//! |-- anyhow
//! |-- opencode_protocol
//! `-- serde_json
//! Used by:
//! `-- rust/xtask/src/codegen/emit.rs
//! Invariants: refs are boxed to permit recursive definitions; unknown alone remains JSON Value.
//! Contract tests: xtask/tests/codegen.rs
use super::{header, pascal, snake};
use anyhow::{Result, bail};
use opencode_protocol::ir::{Field, Optionality, Schema};
use std::collections::BTreeMap;

pub(super) struct Types<'a> {
    pub api: &'a str,
    pub files: &'a mut BTreeMap<String, String>,
    names: BTreeMap<String, Schema>,
}
impl<'a> Types<'a> {
    pub fn new(api: &'a str, files: &'a mut BTreeMap<String, String>) -> Self {
        Self {
            api,
            files,
            names: BTreeMap::new(),
        }
    }
    pub fn path(&self, name: &str) -> String {
        let name = pascal(name);
        let module = snake(&name);
        format!(
            "crate::generated::{}::types::{}::{module}::{name}",
            self.api,
            &module[..1]
        )
    }
    pub fn named(&mut self, name: &str, schema: &Schema) -> Result<String> {
        let name = pascal(name);
        if let Some(existing) = self.names.get(&name) {
            if existing != schema {
                bail!("generated type collision: {name}");
            }
            return Ok(self.path(&name));
        }
        self.names.insert(name.clone(), schema.clone());
        let value = match schema {
            Schema::Struct { fields, index } => self.structure(
                &name,
                fields,
                index.as_ref().map(|(_, value)| value.as_ref()),
            )?,
            Schema::Union { members } => {
                let mut body = format!(
                    "#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]\n#[serde(untagged)]\npub enum {name} {{\n"
                );
                for (index, member) in members.iter().enumerate() {
                    let ty = self.ty(&format!("{name}Variant{index}"), member)?;
                    body.push_str(&format!("    V{index}(Box<{ty}>),\n"));
                }
                body + "}\n"
            }
            Schema::Tuple {
                elements,
                rest: Some(rest),
            } => self.tuple(&name, elements, rest)?,
            Schema::Literal { value } => {
                let literal = serde_json::to_string(value)?;
                format!(
                    "#[derive(Debug, Clone, Copy, PartialEq, Eq)]\npub struct {name};\nimpl serde::Serialize for {name} {{\n    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {{\n        serde::Serialize::serialize(&serde_json::from_str::<serde_json::Value>({literal:?}).map_err(serde::ser::Error::custom)?, serializer)\n    }}\n}}\nimpl<'de> serde::Deserialize<'de> for {name} {{\n    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {{\n        let value = <serde_json::Value as serde::Deserialize>::deserialize(deserializer)?;\n        if value == serde_json::from_str::<serde_json::Value>({literal:?}).map_err(serde::de::Error::custom)? {{ Ok(Self) }} else {{ Err(serde::de::Error::custom(\"unexpected literal\")) }}\n    }}\n}}\n"
                )
            }
            _ => format!(
                "pub type {name} = {};\n",
                self.ty(&format!("{name}Value"), schema)?
            ),
        };
        let module = snake(&name);
        self.files.insert(
            format!("{}/types/{}/{module}.rs", self.api, &module[..1]),
            header(&format!("Protocol type {name}")) + &value,
        );
        Ok(self.path(&name))
    }
    pub fn ty(&mut self, name: &str, schema: &Schema) -> Result<String> {
        Ok(match schema {
            Schema::Ref { name } => format!("Box<{}>", self.path(name)),
            Schema::String { .. } => "String".into(),
            Schema::Number { int, .. } => if *int { "i64" } else { "f64" }.into(),
            Schema::Boolean => "bool".into(),
            Schema::Null => "()".into(),
            Schema::Unknown => "serde_json::Value".into(),
            Schema::Bytes => "Vec<u8>".into(),
            Schema::DateTimeMillis => "i64".into(),
            Schema::Never => "crate::operation::Never".into(),
            Schema::Described { schema, .. } => return self.ty(name, schema),
            Schema::Array { items } => format!("Vec<{}>", self.ty(&format!("{name}Item"), items)?),
            Schema::Record { value, .. } => format!(
                "std::collections::BTreeMap<String, {}>",
                self.ty(&format!("{name}Entry"), value)?
            ),
            Schema::Tuple { elements, rest } => {
                if rest.is_some() {
                    return self.named(name, schema);
                }
                let fields = elements
                    .iter()
                    .enumerate()
                    .map(|(index, schema)| self.ty(&format!("{name}Item{index}"), schema))
                    .collect::<Result<Vec<_>>>()?;
                if fields.is_empty() {
                    "()".into()
                } else {
                    format!("({},)", fields.join(", "))
                }
            }
            Schema::Union { members }
                if members.len() == 2
                    && members.iter().any(|schema| matches!(schema, Schema::Null)) =>
            {
                let schema = members
                    .iter()
                    .find(|schema| !matches!(schema, Schema::Null))
                    .expect("two members include a non-null schema");
                format!("Option<{}>", self.ty(&format!("{name}Present"), schema)?)
            }
            Schema::Struct { .. } | Schema::Union { .. } | Schema::Literal { .. } => {
                self.named(name, schema)?
            }
        })
    }
    fn tuple(&mut self, name: &str, elements: &[Schema], rest: &Schema) -> Result<String> {
        let mut fields = String::new();
        let mut serial = String::new();
        let mut decode = String::new();
        for (index, schema) in elements.iter().enumerate() {
            let ty = self.ty(&format!("{name}Item{index}"), schema)?;
            fields.push_str(&format!("    pub item{index}: {ty},\n"));
            serial.push_str(&format!("        serde::ser::SerializeSeq::serialize_element(&mut sequence, &self.item{index})?;\n"));
            decode.push_str(&format!("            item{index}: serde_json::from_value(values.next().ok_or_else(|| serde::de::Error::custom(\"missing tuple prefix\"))?).map_err(serde::de::Error::custom)?,\n"));
        }
        let ty = self.ty(&format!("{name}Rest"), rest)?;
        Ok(format!(
            "#[derive(Debug, Clone, PartialEq)]\npub struct {name} {{\n{fields}    pub rest: Vec<{ty}>,\n}}\nimpl serde::Serialize for {name} {{\n    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {{\n        let mut sequence = serializer.serialize_seq(Some({} + self.rest.len()))?;\n{serial}        for value in &self.rest {{ serde::ser::SerializeSeq::serialize_element(&mut sequence, value)?; }}\n        serde::ser::SerializeSeq::end(sequence)\n    }}\n}}\nimpl<'de> serde::Deserialize<'de> for {name} {{\n    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {{\n        let mut values = <Vec<serde_json::Value> as serde::Deserialize>::deserialize(deserializer)?.into_iter();\n        Ok(Self {{\n{decode}            rest: values.map(serde_json::from_value).collect::<Result<Vec<_>, _>>().map_err(serde::de::Error::custom)?,\n        }})\n    }}\n}}\n",
            elements.len()
        ))
    }
    fn structure(
        &mut self,
        name: &str,
        fields: &[Field],
        index: Option<&Schema>,
    ) -> Result<String> {
        let mut body = format!(
            "#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]\npub struct {name} {{\n"
        );
        let mut seen = std::collections::BTreeSet::new();
        for field in fields {
            let identifier = snake(&field.name);
            if !seen.insert(identifier.clone()) {
                bail!("field collision in {name}: {}", field.name);
            }
            let ty = self.ty(&format!("{name}{}", pascal(&field.name)), &field.schema)?;
            body.push_str(&format!("    #[serde(rename = {:?})]\n", field.name));
            if field.optionality != Optionality::Required {
                body.push_str("    #[serde(default, skip_serializing_if = \"crate::operation::Optional::is_missing\")]\n");
                body.push_str(&format!(
                    "    pub {identifier}: crate::operation::Optional<{ty}>,\n"
                ));
            } else {
                body.push_str(&format!("    pub {identifier}: {ty},\n"));
            }
        }
        if let Some(schema) = index {
            let ty = self.ty(&format!("{name}Extra"), schema)?;
            body.push_str(&format!("    #[serde(flatten)]\n    pub additional_properties: std::collections::BTreeMap<String, {ty}>,\n"));
        }
        Ok(body + "}\n")
    }
    pub fn finish(&mut self) {
        let mut letters = BTreeMap::<String, Vec<String>>::new();
        for name in self.names.keys() {
            let module = snake(name);
            letters.entry(module[..1].into()).or_default().push(module);
        }
        let mut root = header("generated schema module index");
        for (letter, modules) in letters {
            root.push_str(&format!("pub mod {letter};\n"));
            self.files.insert(
                format!("{}/types/{letter}/mod.rs", self.api),
                header("generated schema family index")
                    + &modules
                        .iter()
                        .map(|module| format!("pub mod {module};\n"))
                        .collect::<String>(),
            );
        }
        self.files
            .insert(format!("{}/types/mod.rs", self.api), root);
    }
}
