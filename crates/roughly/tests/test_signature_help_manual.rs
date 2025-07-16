use roughly::{signature_help, tree, lsp_types::Position};
use ropey::Rope;
use std::collections::HashMap;

#[test]
fn manual_signature_help_test() {
    let content = r#"sum(1, 2, 3)
mean(c(1, 2, 3))
plot(x, y)

base::sum(1, 2)

obj$method(arg1, arg2)

sum(mean(x), median(y))

sum(
  1,
  2,
  3
)"#;
    
    let rope = Rope::from_str(content);
    let tree = tree::parse(&mut tree::new_parser(), content, None);
    
    // Test signature help at different positions
    let test_cases = vec![
        (0, 4),  // Inside sum(1, 2, 3) - first parameter
        (0, 7),  // Inside sum(1, 2, 3) - second parameter  
        (0, 10), // Inside sum(1, 2, 3) - third parameter
        (1, 5),  // Inside mean(c(1, 2, 3)) - first parameter
        (4, 11), // Inside base::sum(1, 2) - first parameter
        (6, 4),  // Inside obj$method(arg1, arg2) - first parameter
        (8, 4),  // Inside sum(mean(x), median(y)) - first parameter
        (10, 2), // Inside multiline sum - first parameter
        (11, 2), // Inside multiline sum - second parameter
    ];
    
    for (line, character) in test_cases {
        let position = Position::new(line, character);
        let result = signature_help::get(position, &rope, &tree, &HashMap::new());
        
        println!("Position {line}:{character} -> {:?}", result.as_ref().map(|s| (s.signatures[0].label.clone(), s.active_parameter)));
        
        // Basic assertions
        if line == 0 && character == 4 {
            assert!(result.is_some());
            let sig = result.unwrap();
            assert_eq!(sig.signatures[0].label, "sum(...)");
            assert_eq!(sig.active_parameter, Some(0));
        } else if line == 0 && character == 7 {
            assert!(result.is_some());
            let sig = result.unwrap();
            assert_eq!(sig.signatures[0].label, "sum(...)");
            assert_eq!(sig.active_parameter, Some(1));
        } else if line == 4 && character == 11 {
            assert!(result.is_some());
            let sig = result.unwrap();
            assert_eq!(sig.signatures[0].label, "sum(...)");
            assert_eq!(sig.active_parameter, Some(0));
        }
    }
}