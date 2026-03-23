# Typing Crate Open Decisions

- Should naming resolve both value names and type names?
  - naming resolves both namespaces
  - naming resolves only value names, and type names resolve elsewhere

- What should naming produce?
  - a new `NamedFile` or `ResolvedFile`
  - HIR plus side tables keyed by stable ids

- What exact environment should typechecking accept as input besides the current file?
  - builtins and imported interfaces only
  - a more explicit precomputed environment shape
