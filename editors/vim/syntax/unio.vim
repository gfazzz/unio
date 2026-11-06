" Vim syntax file for Unio. Keywords follow spec/grammar.ebnf, section L.
if exists("b:current_syntax")
    finish
endif

syn keyword unioControl if else for in while match case return fail try on using break continue pass assert
syn keyword unioOperator and or not is
syn keyword unioStorage function struct enum interface extend let
syn keyword unioModifier public var
syn keyword unioKeyword as returns fails with import from
syn keyword unioReserved uses
syn keyword unioSelf self
syn keyword unioConstant true false none
syn keyword unioType Int Float Bool Text Bytes List Map Set Option Result Tuple Path Function Never Self OrderedFloat Range

" Contextual keywords only in their positions
syn match unioStorage "^\s*\(public\s\+\)\?\zserror\ze\s\+\u"
syn match unioStorage "^\s*\zstest\ze\s\+\""
syn match unioModifier "^\s*\zsunsafe\s\+module\>"
syn match unioModifier "^\s*\(public\s\+\)\?\zsexternal\ze\s\+function\>"
syn match unioModifier "\<named\ze\s\+\l\w*\s\+as\>"

syn match unioFunctionName "\<function\s\+\zs\l\w*"
syn match unioTypeName "\<\u\w*\>"
syn match unioNumber "\<\(0x\x[0-9A-Fa-f_]*\|0b[01][01_]*\|\d[0-9_]*\(\.\d[0-9_]*\)\=\([eE][+-]\=\d[0-9_]*\)\=\)\>"

syn match unioEscape contained "\\\([ntr0\\\"{}]\|u{\x\{1,6}}\)"
syn region unioInterpolation contained matchgroup=unioInterpolationDelimiter start="{" end="}" contains=TOP
syn region unioText start=+"+ skip=+\\.+ end=+"+ contains=unioEscape,unioInterpolation

syn match unioDocComment "##.*$"
syn match unioComment "#[^#].*$\|#$"

hi def link unioControl Statement
hi def link unioOperator Operator
hi def link unioStorage StorageClass
hi def link unioModifier StorageClass
hi def link unioKeyword Keyword
hi def link unioReserved Error
hi def link unioSelf Identifier
hi def link unioConstant Constant
hi def link unioType Type
hi def link unioTypeName Type
hi def link unioFunctionName Function
hi def link unioNumber Number
hi def link unioText String
hi def link unioEscape SpecialChar
hi def link unioInterpolationDelimiter Delimiter
hi def link unioComment Comment
hi def link unioDocComment SpecialComment

let b:current_syntax = "unio"
