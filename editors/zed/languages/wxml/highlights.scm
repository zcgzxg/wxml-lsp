; Comments
(comment) @comment

; Tags
(tag_name) @tag

; Attributes
(attribute_name) @attribute
(attribute_value) @string
(quoted_attribute_value) @string

; WeChat directives and event/data bindings
((attribute_name) @keyword
  (#match? @keyword "^(wx:|bind|catch|mut-bind|model:|data-)"))

; Entity references and interpolation delimiters
(entity) @string.special
(interpolation_start) @punctuation.special
(interpolation_end) @punctuation.special

; Delimiters
[
  "<"
  ">"
  "</"
  "/>"
] @punctuation.bracket

"=" @operator
