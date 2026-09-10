use super::{Document, Emitter, Kind, Parts, View, io};

impl<'tree, 'source> Emitter<'tree, 'source> {
    pub(super) fn binary(&self, view: View<'tree, 'source>) -> io::Result<Document<'source>> {
        let Some(Parts::Binary { operator, .. }) = view.parts() else {
            return Err(io::Error::other("invalid binary expression"));
        };

        let mut operands = Vec::new();
        let mut operators = Vec::new();

        self.operands(
            view,
            precedence(self.text(operator)),
            &mut operands,
            &mut operators,
        );

        let mut operands = operands.into_iter();
        let first = self.node(operands.next().expect("binary operand"))?;
        let count = operators.len();
        let mut rest = Vec::new();
        let mut tail = Document::text("");

        for (index, (operator, operand)) in operators.into_iter().zip(operands).enumerate() {
            let document = self.node(operand)?;

            let hangs = matches!(operand.parts(), Some(Parts::Group { expression }) if expression.kind() == Kind::Conditional)
                && document.width().is_none();

            if index + 1 == count && hangs {
                tail = Document::sequence([
                    Document::text(" "),
                    self.node(operator)?,
                    Document::text(" "),
                    document,
                ]);
            } else {
                rest.extend([
                    Document::Line,
                    self.node(operator)?,
                    Document::text(" "),
                    document,
                ]);
            }
        }

        Ok(Document::sequence([first, Document::sequence(rest).indent(), tail]).group())
    }

    fn operands(
        &self,
        view: View<'tree, 'source>,
        priority: u8,
        operands: &mut Vec<View<'tree, 'source>>,
        operators: &mut Vec<View<'tree, 'source>>,
    ) {
        if let Some(Parts::Binary {
            left,
            operator,
            right,
        }) = view.parts()
            && precedence(self.text(operator)) == priority
            && !self.has_own_comments(view)
        {
            self.operands(left, priority, operands, operators);
            operators.push(operator);
            self.operands(right, priority, operands, operators);
        } else {
            operands.push(view);
        }
    }

    pub(super) fn grouped(
        &self,
        expression: View<'tree, 'source>,
        typeof_expression: bool,
    ) -> io::Result<Document<'source>> {
        let document = self.node(expression)?;

        let opening = Document::text(if typeof_expression { "typeof(" } else { "(" });

        if expression.kind() == Kind::Conditional && document.width().is_none() {
            Ok(Document::sequence([
                opening,
                Document::sequence([Document::Hard, document]).indent(),
                Document::Hard,
                Document::text(")"),
            ]))
        } else {
            Ok(Document::sequence([opening, document, Document::text(")")]))
        }
    }
}

fn precedence(operator: &str) -> u8 {
    match operator {
        "or" => 1,
        "and" => 2,
        "<" | ">" | "<=" | ">=" | "~=" | "==" => 3,
        ".." => 4,
        "+" | "-" => 5,
        "*" | "/" | "//" | "%" => 6,
        "^" => 7,
        _ => 0,
    }
}
