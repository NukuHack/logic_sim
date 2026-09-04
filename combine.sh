#!/bin/bash

if [ -z "$1" ]; then
    echo "Usage: $0 <folder_path> [output_filename]"
    exit 1
fi

if [ ! -d "$1" ]; then
    echo "Error: $1 is not a valid directory"
    exit 1
fi

OUTPUT="${2:-combined.txt}"

# Clear output file
> "$OUTPUT"

# Check if we're in a git repository
if git -C "$1" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    echo "Git repository detected - respecting .gitignore"
    
    # Get all tracked files (respects .gitignore automatically)
    git -C "$1" ls-files | while IFS= read -r file; do
        full_path="$1/$file"
        if [ -f "$full_path" ]; then
            echo "=== $file ===" >> "$OUTPUT"
            cat "$full_path" >> "$OUTPUT"
            echo "" >> "$OUTPUT"
        fi
    done
else
    echo "Not a git repository - including all files"
    # Fallback: include all files (with space handling)
    find "$1" -type f -print0 | while IFS= read -r -d '' file; do
        # Get relative path
        rel_path="${file#$1/}"
        echo "=== $rel_path ===" >> "$OUTPUT"
        cat "$file" >> "$OUTPUT"
        echo "" >> "$OUTPUT"
    done
fi

echo "Combined files into $OUTPUT"
