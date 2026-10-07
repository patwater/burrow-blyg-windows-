//! Checks owner-API traffic against upstream's OpenAPI contract
//! (`tests/fixtures/openapi.json`) and the Worker fork's extensions
//! (`tests/fixtures/extensions.json`). Covers the JSON Schema keywords those
//! files use: `type` (incl. type lists), `$ref`, `properties`, `required`,
//! `additionalProperties`, `enum`, `anyOf`, `items`, `maxItems`, `minLength`,
//! `minimum`, `maximum` and `exclusiveMinimum`. `format` is not checked.

use std::sync::OnceLock;

use serde_json::Value;

pub struct Contract {
    doc: Value,
    /// (METHOD, path template segments, operation)
    ops: Vec<(String, Vec<String>, Value)>,
}

/// Upstream's contract.
pub fn contract() -> &'static Contract {
    static C: OnceLock<Contract> = OnceLock::new();
    C.get_or_init(|| Contract::load(include_str!("../fixtures/openapi.json")))
}

/// The Worker fork's extensions (docs/SERVER.md).
pub fn extensions() -> &'static Contract {
    static C: OnceLock<Contract> = OnceLock::new();
    C.get_or_init(|| Contract::load(include_str!("../fixtures/extensions.json")))
}

impl Contract {
    fn load(text: &str) -> Contract {
        let doc: Value = serde_json::from_str(text).expect("contract JSON");
        let mut ops = vec![];
        for (path, item) in doc["paths"].as_object().unwrap() {
            let segs = path
                .trim_start_matches('/')
                .split('/')
                .map(str::to_string)
                .collect::<Vec<_>>();
            for (method, op) in item.as_object().unwrap() {
                ops.push((method.to_ascii_uppercase(), segs.clone(), op.clone()));
            }
        }
        Contract { doc, ops }
    }
}

fn is_param(seg: &str) -> bool {
    seg.starts_with('{') && seg.ends_with('}')
}

impl Contract {
    /// The operation for `METHOD /path`; with several, the one with the
    /// most literal segments (`/items/{id}` loses to `/items/search`).
    fn op(&self, method: &str, segs: &[&str]) -> Option<&Value> {
        self.ops
            .iter()
            .filter(|(m, t, _)| {
                m == method
                    && t.len() == segs.len()
                    && t.iter().zip(segs).all(|(t, s)| is_param(t) || t == s)
            })
            .max_by_key(|(_, t, _)| t.iter().filter(|s| !is_param(s)).count())
            .map(|(_, _, op)| op)
    }

    /// Whether `METHOD /path` is an operation here.
    pub fn knows(&self, method: &str, segs: &[&str]) -> bool {
        self.op(method, segs).is_some()
    }

    /// Whether this operation declares a reply with `status`.
    pub fn declares(&self, method: &str, segs: &[&str], status: u16) -> bool {
        self.op(method, segs)
            .is_some_and(|op| op["responses"].get(status.to_string()).is_some())
    }

    /// The request: the route exists, its query parameters are declared
    /// and valid, and a JSON body matches the request schema.
    pub fn check_request(
        &self,
        method: &str,
        segs: &[&str],
        query: &[(String, String)],
        content_type: Option<&str>,
        body: &[u8],
    ) -> Result<(), String> {
        let op = self
            .op(method, segs)
            .ok_or_else(|| "not an operation in openapi.json".to_string())?;
        let params = op["parameters"].as_array().cloned().unwrap_or_default();
        for (k, v) in query {
            let p = params
                .iter()
                .find(|p| p["in"] == "query" && p["name"] == k.as_str())
                .ok_or_else(|| format!("undeclared query parameter `{k}`"))?;
            let schema = &p["schema"];
            let value = match serde_json::from_str::<Value>(v) {
                Ok(n @ Value::Number(_)) if self.allows_number(schema) => n,
                _ => Value::String(v.clone()),
            };
            self.validate(schema, &value, &format!("query.{k}"))?;
        }
        let rb = &op["requestBody"];
        let ct = content_type.unwrap_or("").split(';').next().unwrap_or("");
        if body.is_empty() {
            if rb["required"] == true {
                return Err("request body required".into());
            }
            return Ok(());
        }
        if rb.is_null() {
            return Err("this operation takes no request body".into());
        }
        let Some(media) = rb["content"].get(ct) else {
            return Err(format!("request content-type `{ct}` not accepted"));
        };
        if ct == "application/json" {
            let v: Value =
                serde_json::from_slice(body).map_err(|e| format!("request body: {e}"))?;
            self.validate(&media["schema"], &v, "body")?;
        }
        Ok(())
    }

    /// The response: the status is declared for this operation, and the
    /// body matches its schema.
    pub fn check_response(
        &self,
        method: &str,
        segs: &[&str],
        status: u16,
        body: &Value,
    ) -> Result<(), String> {
        let op = self
            .op(method, segs)
            .ok_or_else(|| "not an operation in openapi.json".to_string())?;
        let Some(r) = op["responses"].get(status.to_string()) else {
            return Err(format!("status {status} not declared"));
        };
        let Some(schema) = r["content"]["application/json"].get("schema") else {
            return Ok(());
        };
        self.validate(schema, body, "response")
    }

    fn resolve<'a>(&'a self, s: &'a Value) -> &'a Value {
        match s.get("$ref").and_then(Value::as_str) {
            Some(r) => {
                let ptr = r.trim_start_matches('#');
                self.resolve(self.doc.pointer(ptr).expect("dangling $ref"))
            }
            None => s,
        }
    }

    fn allows_number(&self, s: &Value) -> bool {
        let s = self.resolve(s);
        match &s["type"] {
            Value::String(t) => t == "integer" || t == "number",
            Value::Array(ts) => ts.iter().any(|t| t == "integer" || t == "number"),
            _ => false,
        }
    }

    pub fn validate(&self, s: &Value, v: &Value, at: &str) -> Result<(), String> {
        let s = self.resolve(s);
        if let Some(any) = s.get("anyOf").and_then(Value::as_array) {
            let errs: Vec<String> = any
                .iter()
                .filter_map(|alt| self.validate(alt, v, at).err())
                .collect();
            if errs.len() == any.len() {
                return Err(format!(
                    "{at}: matches no anyOf branch ({})",
                    errs.join(" | ")
                ));
            }
        }
        if let Some(e) = s.get("enum").and_then(Value::as_array)
            && !e.contains(v)
        {
            return Err(format!("{at}: {v} not in {}", Value::Array(e.clone())));
        }
        let types: Vec<&str> = match &s["type"] {
            Value::String(t) => vec![t.as_str()],
            Value::Array(ts) => ts.iter().filter_map(Value::as_str).collect(),
            _ => vec![],
        };
        if !types.is_empty() && !types.iter().any(|t| type_ok(t, v)) {
            return Err(format!("{at}: {v} is not {}", types.join("|")));
        }
        if let Some(n) = v.as_f64() {
            if let Some(m) = s["minimum"].as_f64()
                && n < m
            {
                return Err(format!("{at}: {n} < minimum {m}"));
            }
            if let Some(m) = s["maximum"].as_f64()
                && n > m
            {
                return Err(format!("{at}: {n} > maximum {m}"));
            }
            if let Some(m) = s["exclusiveMinimum"].as_f64()
                && n <= m
            {
                return Err(format!("{at}: {n} <= exclusiveMinimum {m}"));
            }
        }
        if let (Some(t), Some(m)) = (v.as_str(), s["minLength"].as_u64())
            && (t.chars().count() as u64) < m
        {
            return Err(format!("{at}: shorter than {m}"));
        }
        if let (Some(a), Some(m)) = (v.as_array(), s["maxItems"].as_u64())
            && a.len() as u64 > m
        {
            return Err(format!("{at}: more than {m} items"));
        }
        if let (Some(a), Some(items)) = (v.as_array(), s.get("items")) {
            for (i, x) in a.iter().enumerate() {
                self.validate(items, x, &format!("{at}[{i}]"))?;
            }
        }
        if let Some(o) = v.as_object() {
            let props = s["properties"].as_object();
            if let Some(req) = s["required"].as_array() {
                for k in req.iter().filter_map(Value::as_str) {
                    if !o.contains_key(k) {
                        return Err(format!("{at}: missing required `{k}`"));
                    }
                }
            }
            for (k, x) in o {
                match props.and_then(|p| p.get(k)) {
                    Some(ps) => self.validate(ps, x, &format!("{at}.{k}"))?,
                    None => match s.get("additionalProperties") {
                        Some(Value::Bool(false)) => {
                            return Err(format!("{at}: unknown field `{k}`"));
                        }
                        Some(ap @ Value::Object(m)) if !m.is_empty() => {
                            self.validate(ap, x, &format!("{at}.{k}"))?
                        }
                        _ => {}
                    },
                }
            }
        }
        Ok(())
    }
}

fn type_ok(t: &str, v: &Value) -> bool {
    match t {
        "null" => v.is_null(),
        "boolean" => v.is_boolean(),
        "string" => v.is_string(),
        "number" => v.is_number(),
        "integer" => v.as_f64().is_some_and(|n| n.fract() == 0.0),
        "array" => v.is_array(),
        "object" => v.is_object(),
        _ => true,
    }
}
