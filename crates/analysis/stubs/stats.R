# Standard-library stubs: stats package.
#
# Declaration-only R; see base.R for the format. Most statistical functions take optional arguments
# (`na.rm`, `trim`, `probs`, ...) or return data-dependent structures (model objects, data frames),
# neither of which the annotation grammar can describe yet, so they are widened to `Any`. Stubbing them
# anyway means the names resolve (never reported as undefined, never flagged by strict mode) and a call
# yields `Any` rather than a falsely-precise type.

#: Any
sd <- function(x, ...) 0

#: Any
var <- function(x, ...) 0

#: Any
median <- function(x, ...) 0

#: Any
quantile <- function(x, ...) 0

#: Any
cor <- function(x, ...) 0

#: Any
cov <- function(x, ...) 0

#: Any
fivenum <- function(x, ...) 0

#: Any
weighted.mean <- function(x, w, ...) 0

# Model fitting and prediction: the result is a model object whose shape depends on the formula and
# data, so it is not statically typeable.

#: Any
lm <- function(formula, ...) NULL

#: Any
glm <- function(formula, ...) NULL

#: Any
predict <- function(object, ...) NULL

#: Any
residuals <- function(object, ...) NULL

#: Any
fitted <- function(object, ...) NULL

#: Any
coef <- function(object, ...) NULL

# Random generation and distributions: a numeric vector whose length is a runtime value, with optional
# distribution parameters, so widened to Any.

#: Any
rnorm <- function(n, ...) 0

#: Any
runif <- function(n, ...) 0

#: Any
dnorm <- function(x, ...) 0

#: Any
pnorm <- function(q, ...) 0
