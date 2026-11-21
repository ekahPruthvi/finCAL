//! A small arithmetic language for the template's `Cost` cells.
//!
//! Supported syntax: numbers (`1,000`, `~500`), percentages (`5%`), the word
//! `of` as multiplication, identifiers (`SAL`, `Total`, `MTOTAL`, `W1T`,
//! `LEFT_PREV`), parentheses, and `+ - * /`.

use std::collections::HashMap;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
}

#[derive(Debug, Clone)]
pub enum Expr {
    Num(f64),
    Var(String),
    Neg(Box<Expr>),
    Bin(BinOp, Box<Expr>, Box<Expr>),
}

impl Expr {
    pub fn eval(&self, vars: &HashMap<String, f64>) -> Result<f64, String> {
        match self {
            Expr::Num(n) => Ok(*n),
            Expr::Var(name) => vars
                .get(name)
                .copied()
                .ok_or_else(|| format!("unknown name '{}'", name)),
            Expr::Neg(inner) => Ok(-inner.eval(vars)?),
            Expr::Bin(op, lhs, rhs) => {
                let a = lhs.eval(vars)?;
                let b = rhs.eval(vars)?;
                match op {
                    BinOp::Add => Ok(a + b),
                    BinOp::Sub => Ok(a - b),
                    BinOp::Mul => Ok(a * b),
                    BinOp::Div => {
                        if b == 0.0 {
                            Err("division by zero".into())
                        } else {
                            Ok(a / b)
                        }
                    }
                }
            }
        }
    }

}

impl fmt::Display for BinOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
        };
        f.write_str(text)
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Num(f64),
    Ident(String),
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    LParen,
    RParen,
}

/// Rewrites the free-form table text into something the tokenizer understands.
pub fn normalize(src: &str) -> String {
    let mut text = src.to_lowercase();
    for phrase in [
        "left from prev month",
        "left from previous month",
        "left of prev month",
        "left of previous month",
        "prev month left",
        "previous month left",
    ] {
        text = text.replace(phrase, "left_prev");
    }
    // Numeric thousands separators: `1,000` -> `1000`.
    let chars: Vec<char> = text.chars().collect();
    let mut cleaned = String::with_capacity(text.len());
    for (i, c) in chars.iter().enumerate() {
        if *c == ',' {
            let prev_digit = i > 0 && chars[i - 1].is_ascii_digit();
            let next_digit = chars.get(i + 1).map(|n| n.is_ascii_digit()).unwrap_or(false);
            if prev_digit && next_digit {
                continue;
            }
        }
        cleaned.push(*c);
    }
    cleaned
}

fn tokenize(src: &str) -> Result<Vec<Token>, String> {
    let chars: Vec<char> = normalize(src).chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c.is_ascii_digit() || c == '.' {
            let start = i;
            while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                i += 1;
            }
            let text: String = chars[start..i].iter().collect();
            let number = text
                .parse::<f64>()
                .map_err(|_| format!("bad number '{text}'"))?;
            tokens.push(Token::Num(number));
            continue;
        }
        if c.is_alphabetic() || c == '_' {
            let start = i;
            while i < chars.len()
                && (chars[i].is_alphanumeric() || chars[i] == '_' || chars[i] == '#')
            {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            if word == "of" {
                tokens.push(Token::Star);
            } else {
                tokens.push(Token::Ident(sanitize_ident(&word)));
            }
            continue;
        }
        match c {
            '+' => tokens.push(Token::Plus),
            '-' => tokens.push(Token::Minus),
            '*' => tokens.push(Token::Star),
            '/' => tokens.push(Token::Slash),
            '%' => tokens.push(Token::Percent),
            '(' => tokens.push(Token::LParen),
            ')' => tokens.push(Token::RParen),
            '~' | '|' => {}
            other => return Err(format!("unexpected character '{other}'")),
        }
        i += 1;
    }
    Ok(tokens)
}

fn sanitize_ident(word: &str) -> String {
    match word {
        "sal" => "SAL".into(),
        "total" => "TOTAL".into(),
        "mtotal" => "MTOTAL".into(),
        "wnt" => "WNT".into(),
        "left_prev" => "LEFT_PREV".into(),
        other => other.to_uppercase(),
    }
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn next(&mut self) -> Option<Token> {
        let token = self.tokens.get(self.pos).cloned();
        if token.is_some() {
            self.pos += 1;
        }
        token
    }

    fn parse_expression(&mut self) -> Result<Expr, String> {
        let mut lhs = self.parse_term()?;
        while let Some(token) = self.peek() {
            let op = match token {
                Token::Plus => BinOp::Add,
                Token::Minus => BinOp::Sub,
                _ => break,
            };
            self.pos += 1;
            let rhs = self.parse_term()?;
            lhs = Expr::Bin(op, Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    fn parse_term(&mut self) -> Result<Expr, String> {
        let mut lhs = self.parse_unary()?;
        while let Some(token) = self.peek() {
            let op = match token {
                Token::Star => BinOp::Mul,
                Token::Slash => BinOp::Div,
                _ => break,
            };
            self.pos += 1;
            let rhs = self.parse_unary()?;
            lhs = Expr::Bin(op, Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    fn parse_unary(&mut self) -> Result<Expr, String> {
        if matches!(self.peek(), Some(Token::Minus)) {
            self.pos += 1;
            return Ok(Expr::Neg(Box::new(self.parse_unary()?)));
        }
        self.parse_primary()
    }

    fn parse_primary(&mut self) -> Result<Expr, String> {
        let expression = match self.next() {
            Some(Token::Num(n)) => Expr::Num(n),
            Some(Token::Ident(name)) => Expr::Var(name),
            Some(Token::LParen) => {
                let inner = self.parse_expression()?;
                match self.next() {
                    Some(Token::RParen) => inner,
                    _ => return Err("missing ')'".into()),
                }
            }
            Some(other) => return Err(format!("unexpected token {other:?}")),
            None => return Err("incomplete expression".into()),
        };
        // Postfix `%` turns `5%` into `0.05`.
        if matches!(self.peek(), Some(Token::Percent)) {
            self.pos += 1;
            return Ok(Expr::Bin(
                BinOp::Div,
                Box::new(expression),
                Box::new(Expr::Num(100.0)),
            ));
        }
        Ok(expression)
    }
}

pub fn parse(src: &str) -> Result<Expr, String> {
    let tokens = tokenize(src)?;
    if tokens.is_empty() {
        return Err("empty expression".into());
    }
    let mut parser = Parser { tokens, pos: 0 };
    let expression = parser.parse_expression()?;
    if parser.pos != parser.tokens.len() {
        return Err(format!("trailing tokens near {:?}", parser.tokens[parser.pos]));
    }
    Ok(expression)
}

/// True when a cell holds a literal amount such as `500`, `~1,000` or `70.5`,
/// which means the TUI may edit it in place. Formula cells are read-only.
pub fn plain_amount(src: &str) -> Option<f64> {
    let cleaned: String = src
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '~' && *c != ',')
        .collect();
    if cleaned.is_empty() {
        return None;
    }
    if !cleaned
        .chars()
        .all(|c| c.is_ascii_digit() || c == '.' || c == '-')
    {
        return None;
    }
    cleaned.parse::<f64>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars() -> HashMap<String, f64> {
        HashMap::from([("SAL".to_string(), 10_000.0), ("Total".to_string(), 2_000.0)])
    }

    #[test]
    fn parses_percent_of_sal() {
        let expr = parse("5% of SAL").unwrap();
        assert_eq!(expr.eval(&vars()).unwrap(), 500.0);
    }

    #[test]
    fn parses_comma_numbers() {
        let expr = parse("~1,000").unwrap();
        assert_eq!(expr.eval(&vars()).unwrap(), 1000.0);
    }

    #[test]
    fn parses_left_from_prev_month() {
        let mut v = vars();
        v.insert("LEFT_PREV".into(), 250.0);
        let expr = parse("20% of SAL + LEFT from Prev Month").unwrap();
        assert_eq!(expr.eval(&v).unwrap(), 2250.0);
    }

    #[test]
    fn plain_amounts_only() {
        assert_eq!(plain_amount("~1,000"), Some(1000.0));
        assert_eq!(plain_amount(" 70 "), Some(70.0));
        assert_eq!(plain_amount("5% of SAL"), None);
    }
}
