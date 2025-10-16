use anyhow::anyhow;
use serde::{de::Unexpected, Deserialize, Serialize};

use std::{collections::HashMap, fmt::Display, ops::Not, process::Command, sync::Arc};


#[derive(PartialEq, Debug)]
enum TemplateToken {
    Literal(String),
    Variable(String),
}
impl TemplateToken {
    pub fn render(&self, env: &HashMap<String, String>) -> Result<String, anyhow::Error> {
        match self {
            TemplateToken::Literal(s) => Ok(s.clone()),
            TemplateToken::Variable(var) => {
                if let Some(value) = env.get(var) {
                    Ok(value.clone())
                } else {
                    Err(anyhow!("Missing variable '{}'", var))
                }
            }
        }
    }
}

/// A literal instruction to execute, e.g. to quit a binary.
/// 
/// Syntax: `echo %path%`.
#[derive(Clone)]
pub struct Template {
    source: Arc<str>,
    compiled: Arc<[TemplateToken]>,
}
impl Display for Template {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.source.fmt(f)
    }
}

impl Template {
    pub fn render(&self, env: &HashMap<String, String>) -> Result<Command, anyhow::Error> {
        let mut iter = self.compiled.iter();
        let path = iter.next().unwrap().render(env)?; // Invariant: it's always non-empty;
        let mut cmd = Command::new(path);
        for token in iter {
            cmd.arg(token.render(env)?);
        }
        Ok(cmd)
    }
}

impl PartialEq for Template {
    fn eq(&self, other: &Self) -> bool {
        self.source == other.source
    }
}

impl<'a> TryFrom<&'a str> for Template {
    type Error = anyhow::Error;

    fn try_from(source: &'a str) -> Result<Self, Self::Error> {
        let mut tokens = Vec::with_capacity(32);
        let mut current = String::new();
        #[derive(Clone, Copy)]
        enum State {
            Lit,
            Var,
            PercentLit,
        }
        let mut state = State::Lit;
        for c in source.chars() {
            match (c, state) {
                ('%', State::PercentLit) => {
                    // "%%" => "%" escaped within a literal.
                    current.push('%');
                    state = State::Lit;
                }
                (_, State::PercentLit) => {
                    // Literal is over.
                    if current.is_empty().not() {
                        tokens.push(TemplateToken::Literal(current));
                        current = String::new();
                    }
                    current.push(c);
                    state = State::Var;
                }
                ('%', State::Lit) => {
                    // Either an escape or the literal is over.
                    state = State::PercentLit;
                },
                (_, State::Lit) => {
                    // Lit continues.
                    current.push(c);
                }
                ('%', State::Var) => {
                    // Var is complete.
                    state = State::Lit;
                    if current.is_empty().not() {
                        tokens.push(TemplateToken::Variable(current));
                        current = String::new();
                    }
                }
                (_, State::Var) => {
                    // Var continues.
                    current.push(c);
                }
            }
        }
        if current.is_empty().not() {
            match state {
                State::Var => {
                    return Err(anyhow!("unclosed variable name {current}"))
                },
                State::Lit => {
                    tokens.push(TemplateToken::Literal(current))
                }
                State::PercentLit => {
                    return Err(anyhow!("unclosed escape {current}"))
                }
            }
        }
        if tokens.is_empty() {
            return Err(anyhow!("empty command"));
        }
        Ok(Template {
            source: source.to_string().into(),
            compiled: tokens.into(),
        })
    }
}

impl<'a> Deserialize<'a> for Template {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'a> {
        use serde::de::Error;
        let source = String::deserialize(deserializer)?;
        Template::try_from(source.as_str())
            .map_err(|e| D::Error::invalid_value(Unexpected::Str(&format!("{e}")), &"a command template"))
    }
}

impl Serialize for Template {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer {
        self.source.serialize(serializer)
    }
}

impl std::fmt::Debug for Template {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Template").field("source", &self.source).finish()
    }
}

#[cfg(test)]
mod test {
    use crate::config::instruction::{Template, TemplateToken};

    #[test]
    fn test_parse() {
        // Empty string should raise an error.
        let from_empty = Template::try_from("");
        let from_empty_err = from_empty.unwrap_err();
        assert!(format!("{from_empty_err}").contains("empty command"));

        // String without %.
        let from_simple = Template::try_from("abcdef 1234").unwrap();
        assert!(from_simple.source.as_ref() == "abcdef 1234");
        assert!(from_simple.compiled.len() == 1);
        assert_eq!(from_simple.compiled[0], TemplateToken::Literal("abcdef 1234".to_string()));

        // Containing a variable.
        let from_simple = Template::try_from("abc%def 123%4").unwrap();
        assert!(from_simple.source.as_ref() == "abc%def 123%4");
        assert!(from_simple.compiled.len() == 3);
        assert_eq!(from_simple.compiled[0], TemplateToken::Literal("abc".to_string()));
        assert_eq!(from_simple.compiled[1], TemplateToken::Variable("def 123".to_string()));
        assert_eq!(from_simple.compiled[2], TemplateToken::Literal("4".to_string()));

        // Containing two variables.
        let from_simple = Template::try_from("abc%def% %123%4").unwrap();
        assert!(from_simple.source.as_ref() == "abc%def% %123%4");
        assert!(from_simple.compiled.len() == 5);
        assert_eq!(from_simple.compiled[0], TemplateToken::Literal("abc".to_string()));
        assert_eq!(from_simple.compiled[1], TemplateToken::Variable("def".to_string()));
        assert_eq!(from_simple.compiled[2], TemplateToken::Literal(" ".to_string()));
        assert_eq!(from_simple.compiled[3], TemplateToken::Variable("123".to_string()));
        assert_eq!(from_simple.compiled[4], TemplateToken::Literal("4".to_string()));

        // Containing two successive variables.
        let from_simple = Template::try_from("abc%def%%123%4").unwrap();
        assert!(from_simple.source.as_ref() == "abc%def%%123%4");
        assert!(from_simple.compiled.len() == 4);
        assert_eq!(from_simple.compiled[0], TemplateToken::Literal("abc".to_string()));
        assert_eq!(from_simple.compiled[1], TemplateToken::Variable("def".to_string()));
        assert_eq!(from_simple.compiled[2], TemplateToken::Variable("123".to_string()));
        assert_eq!(from_simple.compiled[3], TemplateToken::Literal("4".to_string()));
        
        // Containing an escape.
        let from_simple = Template::try_from("abc%%def 123%%4").unwrap();
        assert!(from_simple.source.as_ref() == "abc%%def 123%%4");
        assert!(from_simple.compiled.len() == 1);
        assert_eq!(from_simple.compiled[0], TemplateToken::Literal("abc%def 123%4".to_string()));
        
        // Starting with a variable.
        let from_simple = Template::try_from("%abc%def 1234").unwrap();
        assert!(from_simple.source.as_ref() == "%abc%def 1234");
        assert!(from_simple.compiled.len() == 2);
        assert_eq!(from_simple.compiled[0], TemplateToken::Variable("abc".to_string()));
        assert_eq!(from_simple.compiled[1], TemplateToken::Literal("def 1234".to_string()));

        // Arbitrary ending with a variable.
        let from_simple = Template::try_from("abcdef 1%234%").unwrap();
        assert!(from_simple.source.as_ref() == "abcdef 1%234%");
        assert!(from_simple.compiled.len() == 2);
        assert_eq!(from_simple.compiled[0], TemplateToken::Literal("abcdef 1".to_string()));        
        assert_eq!(from_simple.compiled[1], TemplateToken::Variable("234".to_string()));
    }
}