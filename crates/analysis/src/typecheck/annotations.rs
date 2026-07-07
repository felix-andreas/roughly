// Annotation application: harvesting `#:` schemes for stub declarations and interface exports,
// applying expression- and binding-level annotations (checked and trusting forms), lowering
// surface types into core types with type-parameter substitutions, and projecting a nominal to
// its representation type where a structural shape is required. The typing-reference sections on
// annotations and named types are the contract.
use {
    super::{InferenceEntry, InferenceError, InferenceState, RECURSION_LIMIT, alias_cycle_error},
    crate::{
        hir::{DefinitionKind, Expression},
        interner::Symbol,
        typecheck::TypeDefinitionEnvironment,
        types::{
            Annotation, AttachedAnnotation, Constraint, CoreType, FunctionType, QuantifiedVariable,
            RecordField, RestParameter, SurfaceType, TypeAnnotationKind, TypeScheme,
        },
    },
    std::collections::{BTreeMap, BTreeSet},
};

impl InferenceState {
    // Harvests a stub annotation's surface type into a `TypeScheme` without inferring any body, used
    // by `StubLibrary::load` to turn declaration-only base stubs into schemes through the ordinary
    // lowering + generalization path.
    pub fn harvest_annotation_scheme(
        &mut self,
        surface_type: &SurfaceType,
        type_definitions: &TypeDefinitionEnvironment,
    ) -> Result<TypeScheme, InferenceError> {
        let core_type = self.lower_annotation_surface_type(surface_type, type_definitions, None)?;
        self.generalize_annotation(core_type)
    }

    // Generalizes a type lowered directly from an annotation (a stub declaration or `@trust` type),
    // where there is no function body to check against. A `<T>` binder lowers to a rigid variable; with
    // no body-inference step to turn it back into an ordinary free variable, ordinary `generalize`
    // (which quantifies only level-scoped unbound variables) would leave it un-quantified and the
    // resulting scheme monomorphic-but-open. Here every rigid variable in the lowered type is a
    // universally quantified parameter, so quantify them alongside the normally generalizable ones.
    pub(super) fn generalize_annotation(
        &mut self,
        core_type: CoreType,
    ) -> Result<TypeScheme, InferenceError> {
        let resolved_type = self.resolve(core_type)?;
        let type_variables = self.free_type_variables_in_core_type(&resolved_type)?;

        let mut quantified_variables = Vec::new();
        for variable in type_variables {
            let Some(entry) = self.entries.get(&variable) else {
                return Err(InferenceError::UnknownInferenceVariable(variable));
            };
            if let InferenceEntry::Unbound { level, constraint } = entry {
                let constraint = *constraint;
                if *level > self.current_level || self.rigid_variables.contains_key(&variable) {
                    quantified_variables.push(QuantifiedVariable::new(variable, constraint));
                }
            }
        }

        Ok(TypeScheme {
            quantified_variables,
            body: resolved_type,
        })
    }

    pub(super) fn apply_annotation(
        &mut self,
        annotation: &AttachedAnnotation,
        inferred_type: CoreType,
        expression: &Expression,
        type_definitions: &TypeDefinitionEnvironment,
    ) -> Result<CoreType, InferenceError> {
        match annotation.annotation() {
            Annotation::Type { kind, surface_type } => {
                let actual_type = self.resolve(inferred_type)?;
                // Naming already diagnosed an unresolved or misapplied type name in the
                // annotation; checking the value against it would only cascade noise.
                let expected_type = match self.lower_annotation_surface_type(
                    surface_type,
                    type_definitions,
                    Some(expression),
                ) {
                    Ok(expected_type) => expected_type,
                    Err(InferenceError::UnresolvedAnnotationType { .. }) => {
                        return Ok(actual_type);
                    }
                    Err(error) => return Err(error),
                };

                match kind {
                    TypeAnnotationKind::Checked => {
                        if self.check_compatibility(
                            actual_type.clone(),
                            expected_type.clone(),
                            type_definitions,
                            Some(expression),
                        )? {
                            Ok(expected_type)
                        } else {
                            // The check failed and its speculative bindings were already reverted by
                            // the `check_compatibility` wrapper. Re-running unification can surface a
                            // more specific cause (occurs check, constraint violation, arity) than the
                            // bare `TypeMismatch`; run it inside a snapshot that is always rolled back
                            // so this error extraction leaves no net mutation.
                            let snapshot = self.snapshot();
                            let unify_result = self.unify_with_context(
                                expected_type.clone(),
                                actual_type.clone(),
                                expression,
                            );
                            self.rollback_to(snapshot);
                            match unify_result {
                                Err(error) => Err(error),
                                Ok(_) => Err(InferenceError::TypeMismatch {
                                    expected: Box::new(expected_type),
                                    actual: Box::new(actual_type),
                                    range: Some(expression.range),
                                    expression_id: Some(expression.id),
                                }),
                            }
                        }
                    }
                    TypeAnnotationKind::UnknownOnly => {
                        if actual_type == CoreType::Unknown {
                            Ok(expected_type)
                        } else {
                            Err(InferenceError::TypeMismatch {
                                expected: Box::new(CoreType::Unknown),
                                actual: Box::new(actual_type),
                                range: Some(expression.range),
                                expression_id: Some(expression.id),
                            })
                        }
                    }
                    TypeAnnotationKind::Trusted => Ok(expected_type),
                }
            }
            Annotation::New { nominal_type } => {
                let lowered_arguments = match nominal_type
                    .type_arguments
                    .iter()
                    .map(|argument| {
                        self.lower_annotation_surface_type(
                            argument,
                            type_definitions,
                            Some(expression),
                        )
                    })
                    .collect::<Result<Vec<_>, _>>()
                {
                    Ok(lowered_arguments) => lowered_arguments,
                    Err(InferenceError::UnresolvedAnnotationType { .. }) => {
                        return self.resolve(inferred_type);
                    }
                    Err(error) => return Err(error),
                };

                // Naming already diagnoses `@new` on unknown names, aliases, and wrong type
                // argument arity; typecheck recovers without piling on a second diagnostic.
                let is_nominal_definition = type_definitions
                    .get(nominal_type.name)
                    .is_some_and(|definition| definition.kind == DefinitionKind::Type);
                if !is_nominal_definition {
                    return self.resolve(inferred_type);
                }

                let Some(representation_type) = self.nominal_representation_type(
                    nominal_type.name,
                    &lowered_arguments,
                    type_definitions,
                    Some(expression),
                )?
                else {
                    return self.resolve(inferred_type);
                };

                let actual_type = self.resolve(inferred_type)?;
                if self.check_compatibility(
                    actual_type.clone(),
                    representation_type.clone(),
                    type_definitions,
                    Some(expression),
                )? {
                    Ok(CoreType::Nominal(nominal_type.name, lowered_arguments))
                } else {
                    Err(InferenceError::TypeMismatch {
                        expected: Box::new(representation_type),
                        actual: Box::new(actual_type),
                        range: Some(expression.range),
                        expression_id: Some(expression.id),
                    })
                }
            }
        }
    }

    pub(super) fn checked_function_annotation(
        &mut self,
        annotation: Option<&AttachedAnnotation>,
        type_definitions: &TypeDefinitionEnvironment,
        expression: &Expression,
    ) -> Result<Option<FunctionType<CoreType>>, InferenceError> {
        let Some(annotation) = annotation else {
            return Ok(None);
        };
        Ok(match annotation.annotation() {
            Annotation::Type {
                kind: TypeAnnotationKind::Checked,
                surface_type,
            } => match self.lower_annotation_surface_type(
                surface_type,
                type_definitions,
                Some(expression),
            ) {
                Ok(CoreType::Function(function_type)) => Some(function_type),
                Ok(_) | Err(InferenceError::UnresolvedAnnotationType { .. }) => None,
                Err(error) => return Err(error),
            },
            Annotation::Type { .. } | Annotation::New { .. } => None,
        })
    }

    pub(super) fn lower_annotation_surface_type(
        &mut self,
        surface_type: &SurfaceType,
        type_definitions: &TypeDefinitionEnvironment,
        expression: Option<&Expression>,
    ) -> Result<CoreType, InferenceError> {
        self.lower_surface_type_with_substitutions(
            surface_type,
            &BTreeMap::new(),
            &mut BTreeSet::new(),
            type_definitions,
            expression,
        )
    }

    // How a lowered `T[]` / `T[named]` element becomes a core type. A concrete atomic or a
    // statically untracked element (`Any`/`Unknown`) forms a vector directly. A type *variable*
    // element — a `<T>` binder used as `T[]` — also forms a vector, and the variable acquires the
    // atomic-element bound. The bound is recorded straight on the entry (not via `constrain_type`)
    // because the element may be a rigid binder: the annotation itself makes the atomic promise
    // here, unlike a function body, which must not add bounds the annotation never declared. Every
    // other element shape is refused: vectors hold atomic elements only, and the historical silent
    // reading of `X[]` as `list[X]` hid the mistake.
    pub(super) fn lower_vector_element(
        &mut self,
        element: CoreType,
        vector: impl Fn(Box<CoreType>) -> CoreType,
    ) -> Result<CoreType, InferenceError> {
        match element {
            CoreType::Scalar(_) | CoreType::Any | CoreType::Unknown => {
                Ok(vector(Box::new(element)))
            }
            CoreType::Variable(variable) => {
                if let Some(InferenceEntry::Unbound { level, constraint }) =
                    self.entries.get(&variable)
                {
                    let raised = InferenceEntry::Unbound {
                        level: *level,
                        constraint: constraint.join(Constraint::AtomicElement),
                    };
                    self.set_entry(variable, raised);
                }
                Ok(vector(Box::new(CoreType::Variable(variable))))
            }
            other_type => Err(InferenceError::InvalidVectorElement {
                element: Box::new(other_type),
            }),
        }
    }

    pub(super) fn lower_surface_type_with_substitutions(
        &mut self,
        surface_type: &SurfaceType,
        substitutions: &BTreeMap<Symbol, CoreType>,
        expanding_aliases: &mut BTreeSet<Symbol>,
        type_definitions: &TypeDefinitionEnvironment,
        expression: Option<&Expression>,
    ) -> Result<CoreType, InferenceError> {
        // Lowering can recurse deeper than the parsed annotation when an alias body expands (see the
        // `SurfaceType::Named` alias arm), so it carries its own guard rather than relying on the
        // type-syntax parser's depth bound.
        if self.recursion_depth >= RECURSION_LIMIT {
            return Err(InferenceError::RecursionLimitExceeded);
        }
        self.recursion_depth += 1;
        let result = self.lower_surface_type_with_substitutions_inner(
            surface_type,
            substitutions,
            expanding_aliases,
            type_definitions,
            expression,
        );
        self.recursion_depth -= 1;
        result
    }

    pub(super) fn lower_surface_type_with_substitutions_inner(
        &mut self,
        surface_type: &SurfaceType,
        substitutions: &BTreeMap<Symbol, CoreType>,
        expanding_aliases: &mut BTreeSet<Symbol>,
        type_definitions: &TypeDefinitionEnvironment,
        expression: Option<&Expression>,
    ) -> Result<CoreType, InferenceError> {
        match surface_type {
            SurfaceType::Any => Ok(CoreType::Any),
            SurfaceType::Unknown => Ok(CoreType::Unknown),
            SurfaceType::Null => Ok(CoreType::Null),
            SurfaceType::Union(members) => {
                let lowered = members
                    .iter()
                    .map(|member| {
                        self.lower_surface_type_with_substitutions(
                            member,
                            substitutions,
                            expanding_aliases,
                            type_definitions,
                            expression,
                        )
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                // Re-normalize: an alias member may have expanded into a type equal to another
                // member, or into a union itself.
                Ok(CoreType::union_of(lowered))
            }
            SurfaceType::Scalar(atomic) => Ok(CoreType::Scalar(*atomic)),
            SurfaceType::Named(name, arguments) => {
                if let Some(core_type) = substitutions.get(name) {
                    if arguments.is_empty() {
                        return Ok(core_type.clone());
                    }
                    // Applying type arguments to a type parameter is a naming diagnostic; lowering
                    // through the (shadowed) global of the same name would silently resolve the
                    // misuse, so it degrades to the same silent skip a wrong arity gets.
                    return Err(InferenceError::UnresolvedAnnotationType { symbol: *name });
                }

                let lowered_arguments = arguments
                    .iter()
                    .map(|argument| {
                        self.lower_surface_type_with_substitutions(
                            argument,
                            substitutions,
                            expanding_aliases,
                            type_definitions,
                            expression,
                        )
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let Some(type_definition) = type_definitions.get(*name).cloned() else {
                    return Err(InferenceError::UnresolvedAnnotationType { symbol: *name });
                };

                // A wrong type-argument count is already a naming diagnostic; lowering the
                // misapplication anyway would check the value against a malformed type and
                // cascade a second error, so it degrades to the same silent skip an unresolved
                // name gets.
                if type_definition.type_parameters.len() != lowered_arguments.len() {
                    return Err(InferenceError::UnresolvedAnnotationType { symbol: *name });
                }

                match type_definition.kind {
                    DefinitionKind::Type => Ok(CoreType::Nominal(*name, lowered_arguments)),
                    DefinitionKind::Alias => {
                        if !expanding_aliases.insert(*name) {
                            return Err(alias_cycle_error(*name, expression));
                        }

                        let lowered_alias = if type_definition.type_parameters.len()
                            != lowered_arguments.len()
                        {
                            Err(InferenceError::UnresolvedAnnotationType { symbol: *name })
                        } else {
                            let mut nested_substitutions = substitutions.clone();
                            for (type_parameter, lowered_argument) in type_definition
                                .type_parameters
                                .iter()
                                .zip(lowered_arguments)
                            {
                                nested_substitutions.insert(*type_parameter, lowered_argument);
                            }

                            match &type_definition.representation {
                                Some(representation) => self.lower_surface_type_with_substitutions(
                                    representation,
                                    &nested_substitutions,
                                    expanding_aliases,
                                    type_definitions,
                                    expression,
                                ),
                                // An alias always carries a representation; an opaque
                                // definition cannot be expanded.
                                None => {
                                    Err(InferenceError::UnresolvedAnnotationType { symbol: *name })
                                }
                            }
                        };

                        expanding_aliases.remove(name);
                        lowered_alias
                    }
                }
            }
            SurfaceType::Vector(inner_type) => {
                let element = self.lower_surface_type_with_substitutions(
                    inner_type,
                    substitutions,
                    expanding_aliases,
                    type_definitions,
                    expression,
                )?;
                self.lower_vector_element(element, CoreType::Vector)
            }
            SurfaceType::NamedVector(inner_type) => {
                let element = self.lower_surface_type_with_substitutions(
                    inner_type,
                    substitutions,
                    expanding_aliases,
                    type_definitions,
                    expression,
                )?;
                self.lower_vector_element(element, CoreType::NamedVector)
            }
            SurfaceType::List(item_type) => Ok(CoreType::List(Box::new(
                self.lower_surface_type_with_substitutions(
                    item_type,
                    substitutions,
                    expanding_aliases,
                    type_definitions,
                    expression,
                )?,
            ))),
            SurfaceType::NamedList(item_type) => Ok(CoreType::NamedList(Box::new(
                self.lower_surface_type_with_substitutions(
                    item_type,
                    substitutions,
                    expanding_aliases,
                    type_definitions,
                    expression,
                )?,
            ))),
            SurfaceType::Record(fields) => Ok(CoreType::Record(
                fields
                    .iter()
                    .map(|field| {
                        Ok(RecordField::with_optional(
                            field.name,
                            self.lower_surface_type_with_substitutions(
                                &field.value,
                                substitutions,
                                expanding_aliases,
                                type_definitions,
                                expression,
                            )?,
                            field.optional,
                        ))
                    })
                    .collect::<Result<Vec<_>, _>>()?,
            )),
            SurfaceType::Tuple(items) => Ok(CoreType::Tuple(
                items
                    .iter()
                    .map(|item| {
                        self.lower_surface_type_with_substitutions(
                            item,
                            substitutions,
                            expanding_aliases,
                            type_definitions,
                            expression,
                        )
                    })
                    .collect::<Result<Vec<_>, _>>()?,
            )),
            SurfaceType::Function(function_type) => {
                let variadic = function_type
                    .variadic
                    .as_ref()
                    .map(|variadic| {
                        Ok::<_, InferenceError>(RestParameter {
                            element: Box::new(self.lower_surface_type_with_substitutions(
                                &variadic.element,
                                substitutions,
                                expanding_aliases,
                                type_definitions,
                                expression,
                            )?),
                            preceding_named: variadic.preceding_named,
                        })
                    })
                    .transpose()?;
                Ok(CoreType::Function(FunctionType::with_variadic(
                    function_type
                        .parameters
                        .iter()
                        .map(|parameter| {
                            self.lower_surface_type_with_substitutions(
                                parameter,
                                substitutions,
                                expanding_aliases,
                                type_definitions,
                                expression,
                            )
                        })
                        .collect::<Result<Vec<_>, _>>()?,
                    function_type
                        .named_parameters
                        .iter()
                        .map(|parameter| {
                            Ok(RecordField::with_optional(
                                parameter.name,
                                self.lower_surface_type_with_substitutions(
                                    &parameter.value,
                                    substitutions,
                                    expanding_aliases,
                                    type_definitions,
                                    expression,
                                )?,
                                parameter.optional,
                            ))
                        })
                        .collect::<Result<Vec<_>, _>>()?,
                    variadic,
                    self.lower_surface_type_with_substitutions(
                        &function_type.return_type,
                        substitutions,
                        expanding_aliases,
                        type_definitions,
                        expression,
                    )?,
                )))
            }
            SurfaceType::Binders(bound_type_parameters, inner_type) => {
                if bound_type_parameters.is_empty() {
                    return self.lower_surface_type_with_substitutions(
                        inner_type,
                        substitutions,
                        expanding_aliases,
                        type_definitions,
                        expression,
                    );
                }

                // A `<T>` binder introduces a universally quantified parameter. While checking a
                // function body against the annotation it must be rigid, so the body cannot bind or
                // constrain it (the body has to work for every T); after the check it generalizes
                // back into the scheme. Instantiating a stored scheme uses ordinary fresh variables,
                // so this only makes annotation binders rigid.
                let mut nested_type_parameters = substitutions.clone();
                for type_parameter in bound_type_parameters {
                    // A declared constraint (`<T: numeric>`) rides on the rigid variable from
                    // creation: the annotation itself promises it, so the body may use the
                    // parameter under that bound and the scheme generalizes back with it.
                    let variable =
                        self.fresh_rigid_variable(type_parameter.name, type_parameter.constraint);
                    nested_type_parameters
                        .insert(type_parameter.name, CoreType::Variable(variable));
                }

                self.lower_surface_type_with_substitutions(
                    inner_type,
                    &nested_type_parameters,
                    expanding_aliases,
                    type_definitions,
                    expression,
                )
            }
        }
    }

    pub(super) fn nominal_representation_type(
        &mut self,
        symbol: Symbol,
        type_arguments: &[CoreType],
        type_definitions: &TypeDefinitionEnvironment,
        expression: Option<&Expression>,
    ) -> Result<Option<CoreType>, InferenceError> {
        let Some(type_definition) = type_definitions.get(symbol).cloned() else {
            return Ok(None);
        };
        if type_definition.kind != DefinitionKind::Type {
            return Ok(None);
        }

        if type_definition.type_parameters.len() != type_arguments.len() {
            return Ok(None);
        }

        let mut substitutions = BTreeMap::new();
        for (type_parameter, type_argument) in type_definition
            .type_parameters
            .iter()
            .zip(type_arguments.iter())
        {
            substitutions.insert(*type_parameter, type_argument.clone());
        }

        let Some(representation) = &type_definition.representation else {
            return Ok(None);
        };
        match self.lower_surface_type_with_substitutions(
            &representation.clone(),
            &substitutions,
            &mut BTreeSet::new(),
            type_definitions,
            expression,
        ) {
            Ok(representation_type) => Ok(Some(representation_type)),
            Err(InferenceError::UnresolvedAnnotationType { .. }) => Ok(None),
            Err(error) => Err(error),
        }
    }
}
