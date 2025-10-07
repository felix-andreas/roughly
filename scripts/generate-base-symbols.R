packages <- c("base", getOption("defaultPackages"))

symbols <- sapply(
    packages,
    \(package) {
        items <- c(package, ls(paste0("package:", package)))
        items[grepl("^[._0-9a-zA-Z]+$", items)] |> paste0(collapse = "\n")
    }
) |> paste0(collapse = "\n---\n")

write(symbols, "base-symbols.txt")
