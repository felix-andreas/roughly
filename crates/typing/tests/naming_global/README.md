# Naming Global Suite

This suite tests the package-global part of `naming`.

It should stay focused on facts that require more than one document:

- package-global value resolution
- latest-definition-wins behavior
- cross-file function and argument resolution
- cross-file type-name resolution

Single-file cases also belong here when they depend on package-global resolution rather than the
local lexical pass. That includes top-level global resolution and type-name resolution.

Use explicit `R/...` paths in the fixture when a document should contribute to the package.
The runner does not rewrite fixture paths.

## Output contract

Fixtures render named HIR for each input file after package-global resolution has run.

That means a snapshot can show:

- a use site in one file resolving to a binding introduced in another file
- later global definitions becoming the winning package binding
- cross-file type references that succeed without producing diagnostics

When a case intentionally targets a naming failure, the fixture should render the diagnostic for the
affected file.
