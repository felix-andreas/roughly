# Naming Local Suite

This suite tests the file-local part of `naming`.

It should stay focused on lexical scope facts that can be understood from one document alone:

- value binding introduction
- lexical shadowing
- resolution of locals and closures within one document
- unresolved globals remaining unresolved until the package-global pass

## Output contract

Fixtures render binding identities directly in the snapshot, for example `x@b0` and `x@b1`.

That output is the main assertion mechanism:

- definition sites show the binding introduced there
- use sites show the binding they resolve to
- shadowing is visible when the same name resolves to different ids in different scopes

When a case intentionally targets a naming failure, the fixture should instead render the naming
output produced by the local pass itself.

## Coverage split

- `scoping.R.test`
  - top-level and block-local lexical behavior
- `functions.R.test`
  - function-local scopes and nested closures
- `loops.R.test`
  - loop bindings and loop-body resolution
