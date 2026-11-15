; Zed injects the whole captured node; it does not read
; `injection.include-children`.

; Mustache expressions inside text and attributes.
((expression) @injection.content
  (#set! injection.language "javascript"))

; Inline <wxs> bodies.
((raw_text) @injection.content
  (#set! injection.language "javascript"))
