# Sample R code
library(ggplot2)

data <- data.frame(
  x = 1:10,
  y = rnorm(10)
)

plot <- ggplot(data, aes(x = x, y = y)) +
  geom_point()

print(plot)
