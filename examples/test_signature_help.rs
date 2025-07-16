use roughly::{signature_help, tree, lsp_types::Position};
use ropey::Rope;
use std::collections::HashMap;

fn main() {
    let content = std::fs::read_to_string("/tmp/test_signature_help.R").unwrap();
    let rope = Rope::from_str(&content);
    let tree = tree::parse(&mut tree::new_parser(), &content, None);
    
    // Test signature help at different positions
    let test_cases = vec![
        (0, 4),  // Inside sum(1, 2, 3) - first parameter
        (0, 7),  // Inside sum(1, 2, 3) - second parameter  
        (0, 10), // Inside sum(1, 2, 3) - third parameter
        (1, 5),  // Inside mean(c(1, 2, 3)) - first parameter
        (4, 11), // Inside base::sum(1, 2) - first parameter
        (7, 4),  // Inside obj$method(arg1, arg2) - first parameter
        (10, 4), // Inside sum(mean(x), median(y)) - first parameter
        (13, 2), // Inside multiline sum - first parameter
        (14, 2), // Inside multiline sum - second parameter
    ];
    
    for (line, character) in test_cases {
        let position = Position::new(line, character);
        let result = signature_help::get(position, &rope, &tree, &HashMap::new());
        
        println!("Position {line}:{character} -> {:?}", result.map(|s| (s.signatures[0].label.clone(), s.active_parameter)));
    }
}