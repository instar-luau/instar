use std::collections::HashMap;
use vermis::{Kind, Parts, View};

#[derive(Default)]
pub(super) struct Binding {
    pub reads: usize,
    pub writes: usize,
    pub mutated: bool,
}

#[derive(Default)]
pub(super) struct Names {
    pub bindings: HashMap<usize, Binding>,
    pub globals: HashMap<String, usize>,
    scopes: Vec<HashMap<String, usize>>,
}

impl Names {
    pub(super) fn resolve(root: View<'_, '_>) -> Self {
        let mut names = Self::default();
        names.scopes.push(HashMap::new());
        names.visit(root);

        names
    }

    fn lookup(&self, name: &str) -> Option<usize> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).copied())
    }

    fn declare(&mut self, name: View<'_, '_>) {
        self.bindings.insert(name.span().start, Binding::default());

        self.scopes
            .last_mut()
            .expect("scope exists")
            .insert(name.text().to_string(), name.span().start);
    }

    fn read(&mut self, name: View<'_, '_>) {
        if let Some(binding) = self
            .lookup(&name.text().to_string())
            .and_then(|index| self.bindings.get_mut(&index))
        {
            binding.reads += 1;
        }
    }

    fn write(&mut self, name: View<'_, '_>) {
        if let Some(binding) = self
            .lookup(&name.text().to_string())
            .and_then(|index| self.bindings.get_mut(&index))
        {
            binding.writes += 1;
        } else {
            *self.globals.entry(name.text().to_string()).or_default() += 1;
        }
    }

    fn mutate(&mut self, mut value: View<'_, '_>) {
        while let Some(Parts::Field { receiver, .. } | Parts::Index { receiver, .. }) =
            value.parts()
        {
            value = receiver;
        }

        if value.kind() == Kind::Name
            && let Some(binding) = self
                .lookup(&value.text().to_string())
                .and_then(|index| self.bindings.get_mut(&index))
        {
            binding.mutated = true;
        }
    }

    fn binding(&mut self, binding: View<'_, '_>) {
        if let Some(Parts::Binding { name, .. }) = binding.parts() {
            self.declare(name);
        }
    }

    fn annotation(&mut self, binding: View<'_, '_>) {
        if let Some(
            Parts::Binding {
                annotation: Some(annotation),
                ..
            }
            | Parts::Variadic {
                annotation: Some(annotation),
            },
        ) = binding.parts()
        {
            self.visit(annotation);
        }
    }

    fn generics(&mut self, generics: Option<View<'_, '_>>) {
        if let Some(generics) = generics {
            for generic in generics.children() {
                if let Some(Parts::Generic {
                    default: Some(default),
                    ..
                }) = generic.parts()
                {
                    self.visit(default);
                }
            }
        }
    }

    fn scoped(&mut self, view: View<'_, '_>) {
        self.scopes.push(HashMap::new());

        for child in view.children() {
            self.visit(child);
        }

        self.scopes.pop();
    }

    fn function(&mut self, view: View<'_, '_>) {
        let Some(Parts::Function {
            name,
            generics,
            parameters,
            returns,
            body,
            ..
        }) = view.parts()
        else {
            unreachable!("function syntax expected");
        };

        self.generics(generics);

        if let Some(name) = name {
            if let Some(Parts::FunctionName { path, method }) = name.parts() {
                if let Some(first) = path.clone().next() {
                    if path.count() == 1 && method.is_none() {
                        if view.kind() == Kind::LocalFunction {
                            self.declare(first);
                        } else {
                            self.write(first);
                        }
                    } else {
                        self.read(first);
                    }
                }
            } else if view.kind() == Kind::LocalFunction {
                self.declare(name);
            }
        }

        for parameter in parameters.children() {
            self.annotation(parameter);
        }

        if let Some(returns) = returns {
            self.visit(returns);
        }

        self.scopes.push(HashMap::new());

        if view.kind() == Kind::Method
            || name.is_some_and(|name| {
                matches!(
                    name.parts(),
                    Some(Parts::FunctionName {
                        method: Some(_),
                        ..
                    })
                )
            })
        {
            self.bindings.insert(view.span().start, Binding::default());

            self.scopes
                .last_mut()
                .expect("scope exists")
                .insert("self".to_owned(), view.span().start);
        }

        for parameter in parameters.children() {
            self.binding(parameter);
        }

        if let Some(body) = body {
            for statement in body.children() {
                self.visit(statement);
            }
        }

        self.scopes.pop();
    }

    fn iteration(&mut self, view: View<'_, '_>) {
        match view.parts() {
            Some(Parts::NumericFor {
                binding,
                start,
                end,
                step,
                body,
            }) => {
                self.visit(start);
                self.visit(end);

                if let Some(step) = step {
                    self.visit(step);
                }

                self.annotation(binding);
                self.scopes.push(HashMap::new());
                self.binding(binding);

                for statement in body.children() {
                    self.visit(statement);
                }

                self.scopes.pop();
            }

            Some(Parts::GenericFor {
                bindings,
                values,
                body,
            }) => {
                for value in values {
                    self.visit(value);
                }

                for binding in bindings.clone() {
                    self.annotation(binding);
                }

                self.scopes.push(HashMap::new());

                for binding in bindings {
                    self.binding(binding);
                }

                for statement in body.children() {
                    self.visit(statement);
                }

                self.scopes.pop();
            }

            Some(Parts::Repeat { body, condition }) => {
                self.scopes.push(HashMap::new());

                for statement in body.children() {
                    self.visit(statement);
                }

                self.visit(condition);
                self.scopes.pop();
            }

            _ => unreachable!("iteration syntax expected"),
        }
    }

    fn call(&mut self, callee: View<'_, '_>, arguments: View<'_, '_>) {
        if let Some(Parts::Field { receiver, name }) = callee.parts()
            && receiver.text() == "table"
            && matches!(
                name.text().to_string().as_str(),
                "insert" | "remove" | "sort" | "clear" | "move"
            )
            && let Some(value) = arguments.children().next()
        {
            self.mutate(value);
        }

        self.visit(callee);
        self.visit(arguments);
    }

    fn visit(&mut self, view: View<'_, '_>) {
        match view.parts() {
            Some(Parts::Block { .. }) => self.scoped(view),

            Some(Parts::Local { bindings, values }) => {
                for value in values {
                    self.visit(value);
                }

                for binding in bindings.clone() {
                    self.annotation(binding);
                }

                for binding in bindings {
                    self.binding(binding);
                }
            }

            Some(Parts::Assignment {
                targets, values, ..
            }) => {
                for value in values {
                    self.visit(value);
                }

                for target in targets {
                    if target.kind() == Kind::Name {
                        self.write(target);

                        if view.kind() == Kind::CompoundAssignment {
                            self.read(target);
                        }
                    } else {
                        self.mutate(target);
                        self.visit(target);
                    }
                }
            }

            Some(Parts::Function { .. }) => self.function(view),

            Some(Parts::NumericFor { .. } | Parts::GenericFor { .. } | Parts::Repeat { .. }) => {
                self.iteration(view);
            }

            Some(Parts::Call { callee, arguments }) => self.call(callee, arguments),

            Some(Parts::MethodCall {
                receiver,
                types,
                arguments,
                ..
            }) => {
                self.visit(receiver);

                if let Some(types) = types {
                    self.visit(types);
                }

                self.visit(arguments);
            }

            Some(Parts::Field { receiver, .. }) => self.visit(receiver),

            Some(Parts::TableField {
                key,
                value,
                indexed,
            }) => {
                if indexed && let Some(key) = key {
                    self.visit(key);
                }

                self.visit(value);
            }

            Some(Parts::TypeName {
                namespace,
                arguments,
                ..
            }) => {
                if let Some(namespace) = namespace {
                    self.read(namespace);
                }

                if let Some(arguments) = arguments {
                    self.visit(arguments);
                }
            }

            Some(Parts::TypeField {
                key, annotation, ..
            }) => {
                if view.kind() == Kind::TypeIndexer {
                    self.visit(key);
                }

                self.visit(annotation);
            }

            Some(Parts::TypeAlias {
                generics,
                annotation,
                ..
            }) => {
                self.generics(generics);
                self.visit(annotation);
            }

            Some(Parts::TypeParameter { annotation, .. }) => self.visit(annotation),
            Some(Parts::Leaf) if view.kind() == Kind::Name => self.read(view),

            _ => {
                for child in view.children() {
                    self.visit(child);
                }
            }
        }
    }
}
