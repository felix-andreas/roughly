use {
    crate::types::{CoreType, FunctionType, InferenceVariableId, RecordField},
    std::collections::{BTreeMap, BTreeSet},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InferenceEntry {
    Unbound,
    Redirect(InferenceVariableId),
    Bound(CoreType),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InferenceState {
    next_variable_id: u32,
    entries: BTreeMap<InferenceVariableId, InferenceEntry>,
}

impl Default for InferenceState {
    fn default() -> Self {
        Self {
            next_variable_id: 0,
            entries: BTreeMap::new(),
        }
    }
}

impl InferenceState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn fresh_variable(&mut self) -> InferenceVariableId {
        let variable = InferenceVariableId(self.next_variable_id);
        self.next_variable_id += 1;
        self.entries.insert(variable, InferenceEntry::Unbound);
        variable
    }

    pub fn entry(&self, variable: InferenceVariableId) -> Option<&InferenceEntry> {
        self.entries.get(&variable)
    }

    pub fn resolve(&mut self, core_type: CoreType) -> Result<CoreType, InferenceError> {
        match core_type {
            CoreType::Variable(variable) => self.resolve_variable(variable),
            CoreType::List(item_type) => {
                let resolved_item_type = self.resolve(*item_type)?;
                Ok(CoreType::List(Box::new(resolved_item_type)))
            }
            CoreType::Record(fields) => {
                let mut resolved_fields = Vec::with_capacity(fields.len());
                for field in fields {
                    resolved_fields.push(RecordField::new(field.name, self.resolve(field.value)?));
                }
                Ok(CoreType::Record(resolved_fields))
            }
            CoreType::Tuple(items) => {
                let mut resolved_items = Vec::with_capacity(items.len());
                for item in items {
                    resolved_items.push(self.resolve(item)?);
                }
                Ok(CoreType::Tuple(resolved_items))
            }
            CoreType::Function(function_type) => {
                let resolved_function_type = self.resolve_function_type(function_type)?;
                Ok(CoreType::Function(resolved_function_type))
            }
            other_type => Ok(other_type),
        }
    }

    pub fn unify(&mut self, left: CoreType, right: CoreType) -> Result<CoreType, InferenceError> {
        let resolved_left = self.resolve(left)?;
        let resolved_right = self.resolve(right)?;

        match (resolved_left, resolved_right) {
            (CoreType::Variable(left_variable), CoreType::Variable(right_variable)) => {
                self.unify_variables(left_variable, right_variable)
            }
            (CoreType::Variable(variable), other_type)
            | (other_type, CoreType::Variable(variable)) => {
                self.bind_variable(variable, other_type.clone())?;
                Ok(other_type)
            }
            (CoreType::Any, other_type) | (other_type, CoreType::Any) => Ok(other_type),
            (CoreType::Unknown, other_type) | (other_type, CoreType::Unknown) => Ok(other_type),
            (CoreType::Null, CoreType::Null) => Ok(CoreType::Null),
            (CoreType::Scalar(left_atomic), CoreType::Scalar(right_atomic))
                if left_atomic == right_atomic =>
            {
                Ok(CoreType::Scalar(left_atomic))
            }
            (CoreType::Vector(left_atomic), CoreType::Vector(right_atomic))
                if left_atomic == right_atomic =>
            {
                Ok(CoreType::Vector(left_atomic))
            }
            (CoreType::List(left_item_type), CoreType::List(right_item_type)) => {
                let unified_item_type = self.unify(*left_item_type, *right_item_type)?;
                Ok(CoreType::List(Box::new(unified_item_type)))
            }
            (CoreType::Tuple(left_items), CoreType::Tuple(right_items)) => {
                self.unify_tuples(left_items, right_items)
            }
            (CoreType::Record(left_fields), CoreType::Record(right_fields)) => {
                self.unify_records(left_fields, right_fields)
            }
            (CoreType::Function(left_function), CoreType::Function(right_function)) => {
                let unified_function = self.unify_functions(left_function, right_function)?;
                Ok(CoreType::Function(unified_function))
            }
            (left_type, right_type) => Err(InferenceError::TypeMismatch {
                expected: left_type,
                actual: right_type,
            }),
        }
    }

    pub fn occurs_in(
        &mut self,
        variable: InferenceVariableId,
        core_type: &CoreType,
    ) -> Result<bool, InferenceError> {
        match self.resolve(core_type.clone())? {
            CoreType::Variable(other_variable) => Ok(variable == other_variable),
            CoreType::List(item_type) => self.occurs_in(variable, &item_type),
            CoreType::Record(fields) => {
                for field in fields {
                    if self.occurs_in(variable, &field.value)? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            CoreType::Tuple(items) => {
                for item in items {
                    if self.occurs_in(variable, &item)? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            CoreType::Function(function_type) => {
                for parameter in function_type.parameters {
                    if self.occurs_in(variable, &parameter)? {
                        return Ok(true);
                    }
                }

                for named_parameter in function_type.named_parameters {
                    if self.occurs_in(variable, &named_parameter.value)? {
                        return Ok(true);
                    }
                }

                self.occurs_in(variable, &function_type.return_type)
            }
            _ => Ok(false),
        }
    }

    fn resolve_variable(
        &mut self,
        variable: InferenceVariableId,
    ) -> Result<CoreType, InferenceError> {
        let Some(entry) = self.entries.get(&variable).cloned() else {
            return Err(InferenceError::UnknownInferenceVariable(variable));
        };

        match entry {
            InferenceEntry::Unbound => Ok(CoreType::Variable(variable)),
            InferenceEntry::Redirect(other_variable) => {
                let resolved_type = self.resolve_variable(other_variable)?;
                self.compress_variable(variable, &resolved_type)?;
                Ok(resolved_type)
            }
            InferenceEntry::Bound(bound_type) => {
                let resolved_type = self.resolve(bound_type)?;
                self.compress_variable(variable, &resolved_type)?;
                Ok(resolved_type)
            }
        }
    }

    fn compress_variable(
        &mut self,
        variable: InferenceVariableId,
        resolved_type: &CoreType,
    ) -> Result<(), InferenceError> {
        let Some(entry) = self.entries.get_mut(&variable) else {
            return Err(InferenceError::UnknownInferenceVariable(variable));
        };

        *entry = match resolved_type {
            CoreType::Variable(other_variable) if *other_variable != variable => {
                InferenceEntry::Redirect(*other_variable)
            }
            other_type => InferenceEntry::Bound(other_type.clone()),
        };

        Ok(())
    }

    fn bind_variable(
        &mut self,
        variable: InferenceVariableId,
        core_type: CoreType,
    ) -> Result<(), InferenceError> {
        if self.occurs_in(variable, &core_type)? {
            return Err(InferenceError::OccursCheckFailed {
                variable,
                in_type: core_type,
            });
        }

        let Some(entry) = self.entries.get_mut(&variable) else {
            return Err(InferenceError::UnknownInferenceVariable(variable));
        };

        *entry = InferenceEntry::Bound(core_type);
        Ok(())
    }

    fn unify_variables(
        &mut self,
        left: InferenceVariableId,
        right: InferenceVariableId,
    ) -> Result<CoreType, InferenceError> {
        if left == right {
            return Ok(CoreType::Variable(left));
        }

        let Some(left_entry) = self.entries.get(&left) else {
            return Err(InferenceError::UnknownInferenceVariable(left));
        };
        if !matches!(left_entry, InferenceEntry::Unbound) {
            let resolved_left = self.resolve_variable(left)?;
            let resolved_right = self.resolve_variable(right)?;
            return self.unify(resolved_left, resolved_right);
        }

        let Some(right_entry) = self.entries.get(&right) else {
            return Err(InferenceError::UnknownInferenceVariable(right));
        };
        if !matches!(right_entry, InferenceEntry::Unbound) {
            let resolved_left = self.resolve_variable(left)?;
            let resolved_right = self.resolve_variable(right)?;
            return self.unify(resolved_left, resolved_right);
        }

        let Some(entry) = self.entries.get_mut(&left) else {
            return Err(InferenceError::UnknownInferenceVariable(left));
        };
        *entry = InferenceEntry::Redirect(right);

        Ok(CoreType::Variable(right))
    }

    fn resolve_function_type(
        &mut self,
        function_type: FunctionType<CoreType>,
    ) -> Result<FunctionType<CoreType>, InferenceError> {
        let mut resolved_parameters = Vec::with_capacity(function_type.parameters.len());
        for parameter in function_type.parameters {
            resolved_parameters.push(self.resolve(parameter)?);
        }

        let mut resolved_named_parameters =
            Vec::with_capacity(function_type.named_parameters.len());
        for named_parameter in function_type.named_parameters {
            resolved_named_parameters.push(RecordField::new(
                named_parameter.name,
                self.resolve(named_parameter.value)?,
            ));
        }

        let resolved_return_type = self.resolve(*function_type.return_type)?;

        Ok(FunctionType::new(
            resolved_parameters,
            resolved_named_parameters,
            resolved_return_type,
        ))
    }

    fn unify_tuples(
        &mut self,
        left_items: Vec<CoreType>,
        right_items: Vec<CoreType>,
    ) -> Result<CoreType, InferenceError> {
        if left_items.len() != right_items.len() {
            return Err(InferenceError::TupleLengthMismatch {
                expected: left_items.len(),
                actual: right_items.len(),
            });
        }

        let mut unified_items = Vec::with_capacity(left_items.len());
        for (left_item, right_item) in left_items.into_iter().zip(right_items) {
            unified_items.push(self.unify(left_item, right_item)?);
        }

        Ok(CoreType::Tuple(unified_items))
    }

    fn unify_records(
        &mut self,
        left_fields: Vec<RecordField<CoreType>>,
        right_fields: Vec<RecordField<CoreType>>,
    ) -> Result<CoreType, InferenceError> {
        let left_names: BTreeSet<_> = left_fields.iter().map(|field| field.name).collect();
        let right_names: BTreeSet<_> = right_fields.iter().map(|field| field.name).collect();

        if left_names != right_names {
            return Err(InferenceError::RecordFieldMismatch {
                expected_fields: left_names.into_iter().collect(),
                actual_fields: right_names.into_iter().collect(),
            });
        }

        let right_by_name: BTreeMap<_, _> = right_fields
            .into_iter()
            .map(|field| (field.name, field.value))
            .collect();

        let mut unified_fields = Vec::with_capacity(left_fields.len());
        for left_field in left_fields {
            let Some(right_value) = right_by_name.get(&left_field.name).cloned() else {
                return Err(InferenceError::RecordFieldMismatch {
                    expected_fields: vec![left_field.name],
                    actual_fields: Vec::new(),
                });
            };

            let unified_value = self.unify(left_field.value, right_value)?;
            unified_fields.push(RecordField::new(left_field.name, unified_value));
        }

        Ok(CoreType::Record(unified_fields))
    }

    fn unify_functions(
        &mut self,
        left_function: FunctionType<CoreType>,
        right_function: FunctionType<CoreType>,
    ) -> Result<FunctionType<CoreType>, InferenceError> {
        if left_function.parameters.len() != right_function.parameters.len() {
            return Err(InferenceError::FunctionArityMismatch {
                expected: left_function.parameters.len(),
                actual: right_function.parameters.len(),
            });
        }

        let left_named_names: BTreeSet<_> = left_function
            .named_parameters
            .iter()
            .map(|parameter| parameter.name)
            .collect();
        let right_named_names: BTreeSet<_> = right_function
            .named_parameters
            .iter()
            .map(|parameter| parameter.name)
            .collect();

        if left_named_names != right_named_names {
            return Err(InferenceError::NamedParameterMismatch {
                expected_parameters: left_named_names.into_iter().collect(),
                actual_parameters: right_named_names.into_iter().collect(),
            });
        }

        let mut unified_parameters = Vec::with_capacity(left_function.parameters.len());
        for (left_parameter, right_parameter) in left_function
            .parameters
            .into_iter()
            .zip(right_function.parameters)
        {
            unified_parameters.push(self.unify(left_parameter, right_parameter)?);
        }

        let right_named_by_name: BTreeMap<_, _> = right_function
            .named_parameters
            .into_iter()
            .map(|parameter| (parameter.name, parameter.value))
            .collect();

        let mut unified_named_parameters = Vec::with_capacity(left_function.named_parameters.len());
        for left_named_parameter in left_function.named_parameters {
            let Some(right_value) = right_named_by_name.get(&left_named_parameter.name).cloned()
            else {
                return Err(InferenceError::NamedParameterMismatch {
                    expected_parameters: vec![left_named_parameter.name],
                    actual_parameters: Vec::new(),
                });
            };

            let unified_value = self.unify(left_named_parameter.value, right_value)?;
            unified_named_parameters
                .push(RecordField::new(left_named_parameter.name, unified_value));
        }

        let unified_return_type =
            self.unify(*left_function.return_type, *right_function.return_type)?;

        Ok(FunctionType::new(
            unified_parameters,
            unified_named_parameters,
            unified_return_type,
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InferenceError {
    UnknownInferenceVariable(InferenceVariableId),
    OccursCheckFailed {
        variable: InferenceVariableId,
        in_type: CoreType,
    },
    TypeMismatch {
        expected: CoreType,
        actual: CoreType,
    },
    TupleLengthMismatch {
        expected: usize,
        actual: usize,
    },
    RecordFieldMismatch {
        expected_fields: Vec<crate::interner::Symbol>,
        actual_fields: Vec<crate::interner::Symbol>,
    },
    FunctionArityMismatch {
        expected: usize,
        actual: usize,
    },
    NamedParameterMismatch {
        expected_parameters: Vec<crate::interner::Symbol>,
        actual_parameters: Vec<crate::interner::Symbol>,
    },
}

#[cfg(test)]
mod tests {
    use {
        super::{InferenceEntry, InferenceError, InferenceState, InferenceVariableId},
        crate::{
            interner::Interner,
            types::{Atomic, CoreType, FunctionType, RecordField},
        },
    };

    #[test]
    fn fresh_variables_start_unbound() {
        let mut inference_state = InferenceState::new();

        let variable = inference_state.fresh_variable();

        assert_eq!(
            inference_state.entry(variable),
            Some(&InferenceEntry::Unbound)
        );
    }

    #[test]
    fn unifying_two_variables_creates_a_redirect() {
        let mut inference_state = InferenceState::new();
        let left = inference_state.fresh_variable();
        let right = inference_state.fresh_variable();

        let unified_type = inference_state
            .unify(CoreType::Variable(left), CoreType::Variable(right))
            .expect("variables should unify");

        assert_eq!(unified_type, CoreType::Variable(right));
        assert_eq!(
            inference_state.entry(left),
            Some(&InferenceEntry::Redirect(right))
        );
    }

    #[test]
    fn resolving_a_redirected_variable_applies_path_compression() {
        let mut inference_state = InferenceState::new();
        let first = inference_state.fresh_variable();
        let second = inference_state.fresh_variable();
        let third = inference_state.fresh_variable();

        inference_state
            .unify(CoreType::Variable(first), CoreType::Variable(second))
            .expect("first and second should unify");
        inference_state
            .unify(CoreType::Variable(second), CoreType::Variable(third))
            .expect("second and third should unify");
        inference_state
            .unify(CoreType::Variable(third), CoreType::Scalar(Atomic::Integer))
            .expect("third should unify with integer");

        let resolved_type = inference_state
            .resolve(CoreType::Variable(first))
            .expect("first variable should resolve");

        assert_eq!(resolved_type, CoreType::Scalar(Atomic::Integer));
        assert_eq!(
            inference_state.entry(first),
            Some(&InferenceEntry::Bound(CoreType::Scalar(Atomic::Integer)))
        );
    }

    #[test]
    fn occurs_check_rejects_recursive_types() {
        let mut inference_state = InferenceState::new();
        let variable = inference_state.fresh_variable();

        let result = inference_state.unify(
            CoreType::Variable(variable),
            CoreType::List(Box::new(CoreType::Variable(variable))),
        );

        assert_eq!(
            result,
            Err(InferenceError::OccursCheckFailed {
                variable,
                in_type: CoreType::List(Box::new(CoreType::Variable(variable))),
            })
        );
    }

    #[test]
    fn scalar_types_unify_only_when_equal() {
        let mut inference_state = InferenceState::new();

        let result = inference_state.unify(
            CoreType::Scalar(Atomic::Integer),
            CoreType::Scalar(Atomic::Double),
        );

        assert_eq!(
            result,
            Err(InferenceError::TypeMismatch {
                expected: CoreType::Scalar(Atomic::Integer),
                actual: CoreType::Scalar(Atomic::Double),
            })
        );
    }

    #[test]
    fn function_types_unify_structurally() {
        let mut inference_state = InferenceState::new();
        let left = CoreType::Function(FunctionType::new(
            vec![CoreType::Scalar(Atomic::Integer)],
            Vec::new(),
            CoreType::Scalar(Atomic::Logical),
        ));
        let right = CoreType::Function(FunctionType::new(
            vec![CoreType::Scalar(Atomic::Integer)],
            Vec::new(),
            CoreType::Scalar(Atomic::Logical),
        ));

        let unified_type = inference_state
            .unify(left, right)
            .expect("functions should unify");

        assert_eq!(
            unified_type,
            CoreType::Function(FunctionType::new(
                vec![CoreType::Scalar(Atomic::Integer)],
                Vec::new(),
                CoreType::Scalar(Atomic::Logical),
            ))
        );
    }

    #[test]
    fn record_types_require_the_same_field_names() {
        let mut inference_state = InferenceState::new();
        let mut interner = Interner::new();
        let left_name = interner.intern("left");
        let right_name = interner.intern("right");

        let left = CoreType::Record(vec![RecordField::new(
            left_name,
            CoreType::Scalar(Atomic::Integer),
        )]);
        let right = CoreType::Record(vec![RecordField::new(
            right_name,
            CoreType::Scalar(Atomic::Integer),
        )]);

        let result = inference_state.unify(left, right);

        assert_eq!(
            result,
            Err(InferenceError::RecordFieldMismatch {
                expected_fields: vec![left_name],
                actual_fields: vec![right_name],
            })
        );
    }

    #[test]
    fn tuple_types_require_the_same_length() {
        let mut inference_state = InferenceState::new();

        let result = inference_state.unify(
            CoreType::Tuple(vec![CoreType::Scalar(Atomic::Integer)]),
            CoreType::Tuple(vec![
                CoreType::Scalar(Atomic::Integer),
                CoreType::Scalar(Atomic::Integer),
            ]),
        );

        assert_eq!(
            result,
            Err(InferenceError::TupleLengthMismatch {
                expected: 1,
                actual: 2,
            })
        );
    }

    #[test]
    fn named_function_parameters_require_the_same_names() {
        let mut inference_state = InferenceState::new();
        let mut interner = Interner::new();
        let left_name = interner.intern("left");
        let right_name = interner.intern("right");

        let left = CoreType::Function(FunctionType::new(
            Vec::new(),
            vec![RecordField::new(
                left_name,
                CoreType::Scalar(Atomic::Integer),
            )],
            CoreType::Scalar(Atomic::Logical),
        ));
        let right = CoreType::Function(FunctionType::new(
            Vec::new(),
            vec![RecordField::new(
                right_name,
                CoreType::Scalar(Atomic::Integer),
            )],
            CoreType::Scalar(Atomic::Logical),
        ));

        let result = inference_state.unify(left, right);

        assert_eq!(
            result,
            Err(InferenceError::NamedParameterMismatch {
                expected_parameters: vec![left_name],
                actual_parameters: vec![right_name],
            })
        );
    }

    #[test]
    fn unknown_inference_variables_are_reported() {
        let mut inference_state = InferenceState::new();

        let result = inference_state.resolve(CoreType::Variable(InferenceVariableId(99)));

        assert_eq!(
            result,
            Err(InferenceError::UnknownInferenceVariable(
                InferenceVariableId(99)
            ))
        );
    }
}
