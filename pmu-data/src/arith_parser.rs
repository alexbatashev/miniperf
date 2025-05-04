use chumsky::prelude::*;

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Binary {
        op: BinOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    Variable(String),
    Constant(String),
    Num(f64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
}

pub fn parse_expr(str_expr: &str) -> Expr {
    let parser = expr().then_ignore(end());

    // TODO handle errors
    parser.parse(str_expr).unwrap()
}

fn ident<'src>() -> impl Parser<'src, &'src str, String> {
    any()
        .filter(|c: &char| c.is_alphanumeric() || *c == '_' || *c == '.')
        .repeated()
        .at_least(1)
        .collect::<String>()
        .and_is(any().filter(|c: &char| c.is_alphabetic()))
}

fn variable<'src>() -> impl Parser<'src, &'src str, Expr> {
    ident().map(Expr::Variable)
}

fn constant<'src>() -> impl Parser<'src, &'src str, Expr> {
    just("$")
        .then(ident())
        .map(|(_, name)| Expr::Constant(name))
}

fn num<'src>() -> impl Parser<'src, &'src str, Expr> {
    text::int(10)
        .then(just('.').then(text::digits(10)).or_not())
        .to_slice()
        .from_str()
        .unwrapped()
        .map(Expr::Num)
}

fn expr<'src>() -> impl Parser<'src, &'src str, Expr> {
    recursive(|expr| {
        let atom = num()
            .or(constant())
            .or(variable())
            .or(expr.clone().delimited_by(just("("), just(")")))
            .padded().boxed();

        let product_op = just("*").padded().to(BinOp::Mul).or(just("/").padded().to(BinOp::Div));
        let product = atom.clone().foldl(product_op.then(atom).repeated(), |lhs, (op, rhs)| {
           Expr::Binary{ op, lhs: Box::new(lhs), rhs: Box::new(rhs) }
        });

        let sum_op = just("+").padded().to(BinOp::Add).or(just("-").padded().to(BinOp::Sub));

        let sum = product.clone().foldl(sum_op.then(product.clone()).repeated(), |lhs, (op, rhs)| {
            Expr::Binary{ op, lhs: Box::new(lhs), rhs: Box::new(rhs) }
        });

        sum
    })
}
