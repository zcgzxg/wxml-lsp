; Structure outline for WXML.
;
; Zed captures `@name` for the label and `@item` for the whole outline entry.
; `element` covers ordinary explicit tags; `self_closing_tag` covers standalone
; and nested self-closing tags. The dedicated template/block/slot/wxs rules
; keep those nodes in the outline even though their start tags have their own
; grammar node names.

(element
  (start_tag
    (tag_name) @name)) @item

(self_closing_tag
  (tag_name) @name) @item

(template_element
  (template_start_tag
    (tag_name) @name)) @item

(block_element
  (block_start_tag
    (tag_name) @name)) @item

(slot_element
  (slot_start_tag
    (tag_name) @name)) @item

(wxs_element
  (wxs_start_tag
    (tag_name) @name)) @item

(import_statement
  (tag_name) @name) @item

(include_statement
  (tag_name) @name) @item
