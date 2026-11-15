(("<" @open ">" @close)
  (#set! rainbow.exclude))

(("<" @open "/>" @close)
  (#set! rainbow.exclude))

(("</" @open ">" @close)
  (#set! rainbow.exclude))

((quoted_attribute_value "\"" @open "\"" @close)
  (#set! rainbow.exclude))

((quoted_attribute_value "'" @open "'" @close)
  (#set! rainbow.exclude))

((interpolation
   (interpolation_start) @open
   (interpolation_end) @close)
  (#set! rainbow.exclude))

((element
   (start_tag) @open
   (end_tag) @close)
  (#set! newline.only)
  (#set! rainbow.exclude))
