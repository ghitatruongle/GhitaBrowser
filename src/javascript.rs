// src/javascript.rs - JavaScript Engine Evaluator with Parentheses & Operator Precedence
#![allow(dead_code)]

use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub enum JsvValue {
    Number(f64),
    Boolean(bool),
    String(String),
    Null,
    Undefined,
    Object(HashMap<String, JsvValue>),
    Function(String, Vec<String>, Box<JsvStatement>),
}

impl JsvValue {
    pub fn number(n: f64) -> Self { JsvValue::Number(n) }
    pub fn boolean(b: bool) -> Self { JsvValue::Boolean(b) }
    pub fn string<S: Into<String>>(s: S) -> Self { JsvValue::String(s.into()) }
    pub fn object() -> Self { JsvValue::Object(HashMap::new()) }

    pub fn as_number(&self) -> Option<f64> {
        match self {
            JsvValue::Number(n) => Some(*n),
            _ => None,
        }
    }

    pub fn as_boolean(&self) -> Option<bool> {
        match self {
            JsvValue::Boolean(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_string(&self) -> Option<&str> {
        match self {
            JsvValue::String(s) => Some(s.as_str()),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum JsvStatement {
    Expr(JsvExpression),
    Var(String, Option<Box<JsvExpression>>),
    If(Box<JsvExpression>, Vec<JsvStatement>, Option<Vec<JsvStatement>>),
    While(Box<JsvExpression>, Vec<JsvStatement>),
    Call(String, Vec<JsvExpression>),
    Return(Option<JsvExpression>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum JsvExpression {
    Number(f64),
    String(String),
    Identifier(String),
    BinaryOp(Box<JsvExpression>, OpKind, Box<JsvExpression>),
    UnaryOp(OpKind, Box<JsvExpression>),
    Call(Box<JsvExpression>, Vec<JsvExpression>),
    Bool(bool),
    Null,
    JsvValue(JsvValue),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OpKind {
    Add, Sub, Mul, Div, Mod, Eq, Neq, Lt, Le, Gt, Ge, And, Or,
}

pub struct JsvEnvironment {
    parent: Option<Box<JsvEnvironment>>,
    pub bindings: HashMap<String, JsvValue>,
}

impl JsvEnvironment {
    pub fn new() -> Self {
        Self {
            parent: None,
            bindings: HashMap::new(),
        }
    }

    pub fn with_parent(parent: JsvEnvironment) -> Self {
        Self {
            parent: Some(Box::new(parent)),
            bindings: HashMap::new(),
        }
    }

    pub fn get(&self, name: &str) -> Option<&JsvValue> {
        if let Some(value) = self.bindings.get(name) {
            return Some(value);
        }
        self.parent.as_ref().and_then(|p| p.get(name))
    }

    pub fn set(&mut self, name: &str, value: JsvValue) {
        self.bindings.insert(name.to_string(), value);
    }

    pub fn define(&mut self, name: &str, value: Option<JsvValue>) {
        if let Some(v) = value {
            self.set(name, v);
        }
    }
}

pub struct JsvEngine {
    pub global_env: JsvEnvironment,
}

impl JsvEngine {
    pub fn new() -> Self {
        let mut env = JsvEnvironment::new();
        env.set("console", JsvValue::object());
        Self { global_env: env }
    }

    pub fn eval(&mut self, code: &str) -> Result<JsvValue, String> {
        let tokens = tokenize(code)?;
        if tokens.is_empty() {
            return Ok(JsvValue::Undefined);
        }
        let expr = parse_expression(&tokens)?;
        self.eval_expr(&expr)
    }

    pub fn execute_script(&mut self, script: &str) -> Result<JsvValue, String> {
        self.eval(script)
    }

    fn eval_expr(&self, expr: &JsvExpression) -> Result<JsvValue, String> {
        match expr {
            JsvExpression::Number(n) => Ok(JsvValue::Number(*n)),
            JsvExpression::String(s) => Ok(JsvValue::String(s.clone())),
            JsvExpression::Bool(b) => Ok(JsvValue::Boolean(*b)),
            JsvExpression::Null => Ok(JsvValue::Null),
            JsvExpression::Identifier(name) => {
                self.global_env.get(name).cloned().ok_or_else(|| format!("Undefined variable: {}", name))
            }
            JsvExpression::BinaryOp(left, op, right) => {
                let l_val = self.eval_expr(left)?;
                let r_val = self.eval_expr(right)?;

                match (l_val, r_val) {
                    (JsvValue::Number(ln), JsvValue::Number(rn)) => match op {
                        OpKind::Add => Ok(JsvValue::Number(ln + rn)),
                        OpKind::Sub => Ok(JsvValue::Number(ln - rn)),
                        OpKind::Mul => Ok(JsvValue::Number(ln * rn)),
                        OpKind::Div => {
                            if rn == 0.0 {
                                Err("Division by zero".to_string())
                            } else {
                                Ok(JsvValue::Number(ln / rn))
                            }
                        }
                        OpKind::Mod => {
                            if rn == 0.0 {
                                Err("Modulo by zero".to_string())
                            } else {
                                Ok(JsvValue::Number(ln % rn))
                            }
                        }
                        OpKind::Eq => Ok(JsvValue::Boolean(ln == rn)),
                        OpKind::Neq => Ok(JsvValue::Boolean(ln != rn)),
                        OpKind::Lt => Ok(JsvValue::Boolean(ln < rn)),
                        OpKind::Le => Ok(JsvValue::Boolean(ln <= rn)),
                        OpKind::Gt => Ok(JsvValue::Boolean(ln > rn)),
                        OpKind::Ge => Ok(JsvValue::Boolean(ln >= rn)),
                        _ => Err("Invalid operator for numbers".to_string()),
                    },
                    (JsvValue::String(ls), JsvValue::String(rs)) => match op {
                        OpKind::Add => Ok(JsvValue::String(format!("{}{}", ls, rs))),
                        OpKind::Eq => Ok(JsvValue::Boolean(ls == rs)),
                        OpKind::Neq => Ok(JsvValue::Boolean(ls != rs)),
                        _ => Err("Invalid operator for strings".to_string()),
                    },
                    _ => Err("Type mismatch in binary operation".to_string()),
                }
            }
            JsvExpression::UnaryOp(op, inner) => {
                let val = self.eval_expr(inner)?;
                match (op, val) {
                    (OpKind::Sub, JsvValue::Number(n)) => Ok(JsvValue::Number(-n)),
                    _ => Err("Invalid unary operation".to_string()),
                }
            }
            JsvExpression::JsvValue(v) => Ok(v.clone()),
            _ => Err("Unsupported expression type".to_string()),
        }
    }
}

fn tokenize(code: &str) -> Result<Vec<String>, String> {
    let mut tokens = Vec::new();
    let chars: Vec<char> = code.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        if chars[i].is_whitespace() {
            i += 1;
            continue;
        }

        if chars[i].is_ascii_digit() || chars[i] == '.' {
            let start = i;
            while i < len && (chars[i].is_ascii_digit() || chars[i] == '.') {
                i += 1;
            }
            tokens.push(chars[start..i].iter().collect());
        } else if chars[i].is_alphabetic() || chars[i] == '_' {
            let start = i;
            while i < len && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            tokens.push(chars[start..i].iter().collect());
        } else if i + 1 < len && matches!(&code[i..i + 2], "==" | "!=" | "<=" | ">=") {
            tokens.push(code[i..i + 2].to_string());
            i += 2;
        } else if ['+', '-', '*', '/', '%', '<', '>', '(', ')'].contains(&chars[i]) {
            tokens.push(chars[i].to_string());
            i += 1;
        } else if chars[i] == '"' || chars[i] == '\'' {
            let quote = chars[i];
            i += 1;
            let start = i;
            while i < len && chars[i] != quote {
                i += 1;
            }
            let s: String = chars[start..i].iter().collect();
            if i < len {
                i += 1;
            }
            tokens.push(format!("\"{}\"", s));
        } else {
            i += 1;
        }
    }

    Ok(tokens)
}

fn parse_expression(tokens: &[String]) -> Result<JsvExpression, String> {
    if tokens.is_empty() {
        return Err("Empty expression".to_string());
    }

    // Strip redundant outer parentheses: e.g. ( 1 + 2 ) -> 1 + 2
    if tokens.first() == Some(&"(".to_string()) && tokens.last() == Some(&")".to_string()) {
        let mut depth = 0;
        let mut wraps_entirely = true;
        for (i, tok) in tokens.iter().enumerate() {
            if tok == "(" { depth += 1; }
            else if tok == ")" { depth -= 1; }
            if depth == 0 && i < tokens.len() - 1 {
                wraps_entirely = false;
                break;
            }
        }
        if wraps_entirely {
            return parse_expression(&tokens[1..tokens.len() - 1]);
        }
    }

    if tokens.len() == 1 {
        let tok = &tokens[0];
        if let Ok(n) = tok.parse::<f64>() {
            return Ok(JsvExpression::Number(n));
        }
        if tok.starts_with('"') && tok.ends_with('"') {
            return Ok(JsvExpression::String(tok[1..tok.len() - 1].to_string()));
        }
        if tok == "true" {
            return Ok(JsvExpression::Bool(true));
        }
        if tok == "false" {
            return Ok(JsvExpression::Bool(false));
        }
        return Ok(JsvExpression::Identifier(tok.clone()));
    }

    // Find operator with lowest precedence outside parentheses (scanned right-to-left)
    let mut op_idx = None;
    let mut min_prec = 1000;
    let mut depth = 0;

    for (i, tok) in tokens.iter().enumerate().rev() {
        if tok == ")" { depth += 1; }
        else if tok == "(" { depth -= 1; }
        else if depth == 0 {
            let prec = match tok.as_str() {
                "==" | "!=" | "<" | "<=" | ">" | ">=" => 1,
                "+" | "-" => 2,
                "*" | "/" | "%" => 3,
                _ => 9999,
            };
            if prec < min_prec {
                min_prec = prec;
                op_idx = Some(i);
            }
        }
    }

    if let Some(idx) = op_idx {
        if idx > 0 && idx < tokens.len() - 1 {
            let left = parse_expression(&tokens[..idx])?;
            let right = parse_expression(&tokens[idx + 1..])?;
            let op = match tokens[idx].as_str() {
                "+" => OpKind::Add,
                "-" => OpKind::Sub,
                "*" => OpKind::Mul,
                "/" => OpKind::Div,
                "%" => OpKind::Mod,
                "==" => OpKind::Eq,
                "!=" => OpKind::Neq,
                "<" => OpKind::Lt,
                "<=" => OpKind::Le,
                ">" => OpKind::Gt,
                ">=" => OpKind::Ge,
                _ => return Err("Unknown operator".to_string()),
            };
            return Ok(JsvExpression::BinaryOp(Box::new(left), op, Box::new(right)));
        }
    }

    if let Ok(n) = tokens[0].parse::<f64>() {
        Ok(JsvExpression::Number(n))
    } else {
        Err("Invalid expression format".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_engine_creation() {
        let engine = JsvEngine::new();
        assert!(engine.global_env.bindings.contains_key("console"));
    }

    #[test]
    fn test_eval_basic() {
        let mut engine = JsvEngine::new();
        let result = engine.eval("1 + 1");
        assert!(result.is_ok());
        assert_eq!(result.unwrap().as_number(), Some(2.0));
    }

    #[test]
    fn test_eval_precedence_and_parens() {
        let mut engine = JsvEngine::new();
        let res1 = engine.eval("2 + 3 * 4").unwrap();
        assert_eq!(res1.as_number(), Some(14.0));

        let res2 = engine.eval("(2 + 3) * 4").unwrap();
        assert_eq!(res2.as_number(), Some(20.0));
    }
}