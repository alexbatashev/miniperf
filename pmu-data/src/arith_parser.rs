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
}

/// Parses a valid TMA arithmetic formula.
///
/// # Panics
///
/// Panics when `expression` is not a valid formula.
pub fn parse_expr(expression: &str) -> Expr {
    Parser::new(expression)
        .parse()
        .unwrap_or_else(|error| panic!("invalid TMA formula '{expression}': {error}"))
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
        let expression = self.sum()?;
        self.whitespace();
        if let Some(character) = self.peek() {
            return Err(format!("unexpected '{character}' at byte {}", self.offset));
        }
        Ok(expression)
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
        self.identifier().map(Expr::Variable)
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
}
