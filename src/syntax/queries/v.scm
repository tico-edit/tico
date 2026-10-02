; From vlang/v-analyzer (https://github.com/vlang/v-analyzer, commit
; 925d457, the revision the tree-sitter-vlang crate pins),
; tree_sitter_v/queries/helix.highlights.scm. MIT.
;
; Modified for tico:
; - Reordered. Helix lets the first matching pattern win, tico paints later
;   captures over earlier ones, so the generic selector-field pattern
;   (@variable.other.member) comes before the method-call one, and the
;   string patterns come before the interpolation punctuation.
; - `(array_creation) @punctuation.bracket` is dropped: it painted a whole
;   array literal, elements included, as punctuation. The brackets are
;   already captured by the "[" "]" pattern below.
; - `(string_interpolation)` is dropped from the @string list, and
;   `(interpolation_expression) @none` replaced by @variable, so the code
;   inside `${...}` doesn't read as string text.
; - `(float_literal) @constant.numeric.float` added; `(short_lambda ...)`
;   captures @variable.parameter rather than the nonstandard @parameter.
; - `(attribute "]" @attribute)` added at the end, so an attribute's
;   closing bracket matches its opening `@[`.

[
 (line_comment)
 (block_comment)
 (shebang)
] @comment

(module_clause
 (identifier) @namespace)

(import_path
 (import_name) @namespace)

(import_alias
 (import_name) @namespace)

(enum_fetch
 (reference_expression) @constant)

(enum_field_definition
 (identifier) @constant)

(global_var_definition
 (identifier) @constant)

(compile_time_if_expression
 condition: (reference_expression) @constant)

(compile_time_if_expression
 condition: (binary_expression
              left: (reference_expression) @constant
              right: (reference_expression) @constant))

(compile_time_if_expression
 condition: (binary_expression
              left: (reference_expression) @constant
              right: (unary_expression (reference_expression) @constant)))

(label_reference) @label

(parameter_declaration
 name: (identifier) @variable.parameter)
(receiver
 name: (identifier) @variable.parameter)
(function_declaration
 name: (identifier) @function)
(function_declaration
 receiver: (receiver)
 name: (identifier) @function.method)
(interface_method_definition
 name: (identifier) @function.method)

(short_lambda
  (reference_expression) @variable.parameter)

(struct_field_declaration
 name: (identifier) @variable.other.member)

(field_name) @variable.other.member

(selector_expression
 field: (reference_expression) @variable.other.member)

(call_expression
  name: (selector_expression
  field: (reference_expression) @function.method))

(call_expression
 name: (reference_expression) @function)

(struct_declaration
 name: (identifier) @type)

(enum_declaration
 name: (identifier) @type)

(interface_declaration
 name: (identifier) @type)

(type_declaration
 name: (identifier) @type)

(int_literal) @constant.numeric.integer
(float_literal) @constant.numeric.float

[
 (c_string_literal)
 (raw_string_literal)
 (interpreted_string_literal)
 (rune_literal)
] @string

(escape_sequence) @constant.character.escape

(interpolation_expression) @variable

(string_interpolation
 (interpolation_opening) @punctuation.bracket
 (interpolation_closing) @punctuation.bracket)

(attribute) @attribute

[
 (type_reference_expression)
 ] @type

[
 (true)
 (false)
] @constant.builtin.boolean

[
  "pub"
  "assert"
  "asm"
  "defer"
  "unsafe"
  "sql"
  (nil)
  (none)
] @keyword

[
  "interface"
  "enum"
  "type"
  "union"
  "struct"
  "module"
] @keyword.storage.type

[
  "static"
  "const"
  "__global"
] @keyword.storage.modifier

[
  "mut"
] @keyword.storage.modifier.mut

[
  "shared"
  "lock"
  "rlock"
  "spawn"
  "break"
  "continue"
  "go"
] @keyword.control

[
  "if"
  "$if"
  "select"
  "else"
  "$else"
  "match"
] @keyword.control.conditional

[
  "for"
] @keyword.control.repeat

[
  "goto"
  "return"
] @keyword.control.return

[
  "fn"
] @keyword.control.function


[
  "import"
] @keyword.control.import

[
  "as"
  "in"
  "is"
  "or"
] @keyword.operator

[
 "."
 ","
 ":"
 ";"
] @punctuation.delimiter

[
 "("
 ")"
 "{"
 "}"
 "["
 "]"
] @punctuation.bracket

[
 "++"
 "--"

 "+"
 "-"
 "*"
 "/"
 "%"

 "~"
 "&"
 "|"
 "^"

 "!"
 "&&"
 "||"
 "!="

 "<<"
 ">>"

 "<"
 ">"
 "<="
 ">="

 "+="
 "-="
 "*="
 "/="
 "&="
 "|="
 "^="
 "<<="
 ">>="

 "="
 ":="
 "=="

 "?"
 "<-"
 "$"
 ".."
 "..."
] @operator

; An attribute's closing bracket belongs to the attribute (`@[` is a single
; token, so only the `]` would otherwise be painted as a bracket).
(attribute "]" @attribute)
