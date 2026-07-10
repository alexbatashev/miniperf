/// Parsed arithmetic expression used by a TMA metric.
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    /// A binary arithmetic operation.
    Binary {
        /// Operator applied to both operands.
        op: BinOp,
        /// Left-hand operand.
        lhs: Box<Expr>,
        /// Right-hand operand.
        rhs: Box<Expr>,
    },
    /// A PMU event or another metric.
    Variable(String),
    /// A scenario constant, written with a `$` prefix.
    Constant(String),
    /// A numeric literal.
    Num(f64),
    /// Function call used by vendor metric JSON (`min`, `max`, `abs`, `if`).
    Call {
        /// Function identifier.
        name: String,
        /// Positional arguments.
        args: Vec<Expr>,
    },
}

/// Arithmetic binary operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    /// Addition.
    Add,
    /// Subtraction.
    Sub,
    /// Multiplication.
    Mul,
    /// Division.
    Div,
    /// Equality comparison.
    Eq,
    /// Less-than comparison.
    Lt,
    /// Less-than-or-equal comparison.
    Le,
    /// Greater-than comparison.
    Gt,
    /// Greater-than-or-equal comparison.
    Ge,
}

/// Parses a valid TMA arithmetic formula.
///
/// # Panics
///
/// Panics when `expression` is not a valid formula.
pub fn parse_expr(expression: &str) -> Expr {
    try_parse_expr(expression)
        .unwrap_or_else(|error| panic!("invalid TMA formula '{expression}': {error}"))
}

/// Parses a TMA formula without panicking, suitable for imported metric data.
pub fn try_parse_expr(expression: &str) -> Result<Expr, String> {
    Parser::new(expression).parse()
}

struct Parser<'a> {
    input: &'a str,
    offset: usize,
}

impl<'a> Parser<'a> {
    fn new(input: &'a str) -> Self {
        Self { input, offset: 0 }
    }

    fn parse(mut self) -> Result<Expr, String> {
        let expression = self.comparison()?;
        self.whitespace();
        if let Some(character) = self.peek() {
            return Err(format!("unexpected '{character}' at byte {}", self.offset));
        }
        Ok(expression)
    }

    fn comparison(&mut self) -> Result<Expr, String> {
        let lhs = self.sum()?;
        self.whitespace();
        let op = if self.input[self.offset..].starts_with(">=") {
            self.offset += 2;
            Some(BinOp::Ge)
        } else if self.input[self.offset..].starts_with("<=") {
            self.offset += 2;
            Some(BinOp::Le)
        } else if self.input[self.offset..].starts_with("==") {
            self.offset += 2;
            Some(BinOp::Eq)
        } else if self.consume('>') {
            Some(BinOp::Gt)
        } else if self.consume('<') {
            Some(BinOp::Lt)
        } else {
            None
        };
        match op {
            Some(op) => Ok(Expr::Binary {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(self.sum()?),
            }),
            None => Ok(lhs),
        }
    }

    fn sum(&mut self) -> Result<Expr, String> {
        let mut lhs = self.product()?;
        loop {
            self.whitespace();
            let op = if self.consume('+') {
                BinOp::Add
            } else if self.consume('-') {
                BinOp::Sub
            } else {
                return Ok(lhs);
            };
            lhs = Expr::Binary {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(self.product()?),
            };
        }
    }

    fn product(&mut self) -> Result<Expr, String> {
        let mut lhs = self.atom()?;
        loop {
            self.whitespace();
            let op = if self.consume('*') {
                BinOp::Mul
            } else if self.consume('/') {
                BinOp::Div
            } else {
                return Ok(lhs);
            };
            lhs = Expr::Binary {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(self.atom()?),
            };
        }
    }

    fn atom(&mut self) -> Result<Expr, String> {
        self.whitespace();
        if self.consume('(') {
            let expression = self.sum()?;
            self.whitespace();
            if !self.consume(')') {
                return Err(format!("missing ')' at byte {}", self.offset));
            }
            return Ok(expression);
        }
        if self.consume('$') {
            return self.identifier().map(Expr::Constant);
        }
        let Some(character) = self.peek() else {
            return Err("unexpected end of formula".to_string());
        };
        if character.is_ascii_digit() || character == '.' {
            return self.number().map(Expr::Num);
        }
        let identifier = self.identifier()?;
        self.whitespace();
        if !self.consume('(') {
            return Ok(Expr::Variable(identifier));
        }
        let mut args = Vec::new();
        self.whitespace();
        if !self.consume(')') {
            loop {
                args.push(self.comparison()?);
                self.whitespace();
                if self.consume(')') {
                    break;
                }
                if !self.consume(',') {
                    return Err(format!("expected ',' or ')' at byte {}", self.offset));
                }
            }
        }
        Ok(Expr::Call {
            name: identifier,
            args,
        })
    }

    fn identifier(&mut self) -> Result<String, String> {
        let start = self.offset;
        while self.peek().is_some_and(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '_' | '.')
        }) {
            self.bump();
        }
        if self.offset == start
            || !self.input[start..self.offset]
                .chars()
                .any(char::is_alphabetic)
        {
            return Err(format!("expected identifier at byte {start}"));
        }
        Ok(self.input[start..self.offset].to_string())
    }

    fn number(&mut self) -> Result<f64, String> {
        let start = self.offset;
        while self
            .peek()
            .is_some_and(|character| character.is_ascii_digit() || character == '.')
        {
            self.bump();
        }
        self.input[start..self.offset]
            .parse()
            .map_err(|_| format!("invalid number at byte {start}"))
    }

    fn whitespace(&mut self) {
        while self.peek().is_some_and(char::is_whitespace) {
            self.bump();
        }
    }

    fn consume(&mut self, expected: char) -> bool {
        if self.peek() == Some(expected) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn peek(&self) -> Option<char> {
        self.input[self.offset..].chars().next()
    }

    fn bump(&mut self) {
        if let Some(character) = self.peek() {
            self.offset += character.len_utf8();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tma_formula_with_precedence() {
        assert_eq!(
            parse_expr("event / ($width * cycles)"),
            Expr::Binary {
                op: BinOp::Div,
                lhs: Box::new(Expr::Variable("event".to_string())),
                rhs: Box::new(Expr::Binary {
                    op: BinOp::Mul,
                    lhs: Box::new(Expr::Constant("width".to_string())),
                    rhs: Box::new(Expr::Variable("cycles".to_string())),
                }),
            }
        );
    }

    #[test]
    fn parses_vendor_functions() {
        assert_eq!(
            try_parse_expr("max(0, abs(a - b))").unwrap(),
            Expr::Call {
                name: "max".to_owned(),
                args: vec![
                    Expr::Num(0.0),
                    Expr::Call {
                        name: "abs".to_owned(),
                        args: vec![Expr::Binary {
                            op: BinOp::Sub,
                            lhs: Box::new(Expr::Variable("a".to_owned())),
                            rhs: Box::new(Expr::Variable("b".to_owned()))
                        }]
                    }
                ]
            }
        );
    }

    #[test]
    fn parses_conditional_comparison() {
        assert!(matches!(
            try_parse_expr("if(a >= b, 1, 0)").unwrap(),
            Expr::Call { name, args } if name == "if" && matches!(args[0], Expr::Binary { op: BinOp::Ge, .. })
        ));
    }
}
