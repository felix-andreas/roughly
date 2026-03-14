use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Symbol(u32);

impl Symbol {
    pub fn as_u32(self) -> u32 {
        self.0
    }
}

#[derive(Debug, Default)]
pub struct Interner {
    symbols_by_text: HashMap<String, Symbol>,
    text_by_symbol: Vec<String>,
}

impl Interner {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn intern(&mut self, text: &str) -> Symbol {
        if let Some(symbol) = self.symbols_by_text.get(text).copied() {
            return symbol;
        }

        let symbol_index = self.text_by_symbol.len();
        let symbol =
            Symbol(u32::try_from(symbol_index).expect("symbol interner exceeded u32 capacity"));

        let owned_text = text.to_owned();
        self.symbols_by_text.insert(owned_text.clone(), symbol);
        self.text_by_symbol.push(owned_text);

        symbol
    }

    pub fn resolve(&self, symbol: Symbol) -> Option<&str> {
        let symbol_index = usize::try_from(symbol.0).ok()?;
        self.text_by_symbol.get(symbol_index).map(String::as_str)
    }

    pub fn contains(&self, text: &str) -> bool {
        self.symbols_by_text.contains_key(text)
    }

    pub fn len(&self) -> usize {
        self.text_by_symbol.len()
    }

    pub fn is_empty(&self) -> bool {
        self.text_by_symbol.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::{Interner, Symbol};

    #[test]
    fn interning_the_same_text_returns_the_same_symbol() {
        let mut interner = Interner::new();

        let first_symbol = interner.intern("value");
        let second_symbol = interner.intern("value");

        assert_eq!(first_symbol, second_symbol);
        assert_eq!(interner.len(), 1);
    }

    #[test]
    fn interning_different_text_returns_different_symbols() {
        let mut interner = Interner::new();

        let first_symbol = interner.intern("left");
        let second_symbol = interner.intern("right");

        assert_ne!(first_symbol, second_symbol);
        assert_eq!(interner.len(), 2);
    }

    #[test]
    fn resolve_returns_the_original_text() {
        let mut interner = Interner::new();

        let symbol = interner.intern("identity");

        assert_eq!(interner.resolve(symbol), Some("identity"));
    }

    #[test]
    fn resolve_returns_none_for_unknown_symbol() {
        let interner = Interner::new();

        assert_eq!(interner.resolve(Symbol(0)), None);
    }
}
