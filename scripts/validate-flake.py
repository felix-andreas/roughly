#!/usr/bin/env python3
"""
Validate the flake.nix syntax and structure.
This script performs basic validation that can be run without Nix installed.
"""

import re
import sys
from pathlib import Path

def validate_flake_syntax(flake_path):
    """Validate basic syntax of flake.nix"""
    print(f"Validating {flake_path}...")
    
    with open(flake_path, 'r') as f:
        content = f.read()
    
    errors = []
    warnings = []
    
    # Check balanced delimiters
    delimiters = {'{': '}', '[': ']', '(': ')'}
    stack = []
    
    for i, char in enumerate(content):
        if char in delimiters:
            stack.append((char, i))
        elif char in delimiters.values():
            if not stack:
                errors.append(f"Unmatched closing delimiter '{char}' at position {i}")
                continue
            open_char, open_pos = stack.pop()
            if delimiters[open_char] != char:
                errors.append(f"Mismatched delimiter: '{open_char}' at {open_pos} closed by '{char}' at {i}")
    
    if stack:
        for open_char, pos in stack:
            errors.append(f"Unclosed delimiter '{open_char}' at position {pos}")
    
    # Check for required sections
    required_sections = ['inputs', 'outputs', 'packages', 'devShells']
    for section in required_sections:
        if section not in content:
            errors.append(f"Missing required section: {section}")
    
    # Check for crane input
    if 'crane' not in content:
        warnings.append("crane input not found")
    
    # Check for macOS-specific dependencies
    macos_deps = ['darwin.apple_sdk.frameworks.Security', 'libiconv']
    has_macos_deps = any(dep in content for dep in macos_deps)
    if not has_macos_deps:
        warnings.append("No macOS-specific dependencies found")
    
    # Check for buildDepsOnly (crane best practice)
    if 'buildDepsOnly' not in content:
        warnings.append("buildDepsOnly not used (crane best practice for caching)")
    
    return errors, warnings

def main():
    flake_path = Path(__file__).parent.parent / "flake.nix"
    
    if not flake_path.exists():
        print(f"Error: {flake_path} not found")
        sys.exit(1)
    
    errors, warnings = validate_flake_syntax(flake_path)
    
    if errors:
        print("\nERRORS:")
        for error in errors:
            print(f"  ❌ {error}")
    
    if warnings:
        print("\nWARNINGS:")
        for warning in warnings:
            print(f"  ⚠️  {warning}")
    
    if not errors and not warnings:
        print("✅ flake.nix validation passed!")
    elif not errors:
        print("✅ flake.nix syntax is valid (with warnings)")
    else:
        print("❌ flake.nix validation failed")
        sys.exit(1)

if __name__ == "__main__":
    main()