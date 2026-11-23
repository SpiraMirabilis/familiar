" Vim syntax file
" Language: MOO (Stunt / LambdaMOO)
" Built from Familiar's GtkSourceView spec (crates/gtk/data/language-specs/moo.lang): LambdaMOO
" plus try/except, break/continue, maps, bitwise operators, /* comments */, $#colour literals,
" b"bytes" literals and string escapes.
" Install: copy to ~/.config/nvim/syntax/ (Neovim) or ~/.vim/syntax/ (Vim). It's used instead of
" the moo.vim that comes with them.

if exists("b:current_syntax")
  finish
endif
let s:cpo_save = &cpo
set cpo&vim

" MOO keywords and names are case-insensitive.
syn case ignore
syn iskeyword @,48-57,_

" Defined first, so everything after wins where they overlap.
syn match mooOperator ,\v-\>|\=\>|\.\.|\&\&|\|\||\=\=|!\=|\<\=|\>\=|\&\.|\|\.|\^\.|\<\<|\>\>|[-+*/%^!~<>=?|@`'$],

" Not after a single dot, but after .. as in x[1..5].
syn match mooInteger /\v([A-Za-z0-9_$#])@<!((\.\.)@<=|\.@<!)\d+([A-Za-z0-9_])@!/
" 1.5, 1., .5 and 1e3, but not the 1 in the range 1..5.
syn match mooFloat /\v([A-Za-z0-9_.$#])@<!(\d+\.\d+(e[+-]?\d+)?|\d+\.([.0-9])@!(e[+-]?\d+)?|\.\d+(e[+-]?\d+)?|\d+e[+-]?\d+)([A-Za-z0-9_])@!/

syn match mooObject /#-\=\d\+/
syn match mooSysRef /\$\h\w*/
syn match mooColour /\$#\w\+/

syn keyword mooConditional if else elseif endif
syn keyword mooRepeat for in endfor while endwhile
syn keyword mooException try except finally endtry
syn keyword mooStatement return break continue fork endfork
syn keyword mooKeyword ANY

syn keyword mooError E_NONE E_TYPE E_DIV E_PERM E_PROPNF E_VERBNF E_VARNF E_INVIND E_RECMOVE
      \ E_MAXREC E_RANGE E_ARGS E_NACC E_INVARG E_QUOTA E_FLOAT E_FILE E_EXEC E_INTRPT

" Built-in variables and type constants, but not as property or verb names like x.player.
syn match mooTypeName /\v([.:$])@<!<(NUM|OBJ|STR|LIST|ERR|INT|FLOAT|MAP|ANON|BYTES|COLOR)>/
syn match mooVariable /\v([.:$])@<!<(player|this|caller|verb|args|argstr|dobj|dobjstr|prepstr|iobj|iobjstr)>/

" Built-in functions are only highlighted where they're called.
syn match mooBuiltin /\v<(abs|acos|add_property|add_verb|ancestors|asin|atan|binary_hash|binary_hmac|binary_to_bytes|boot_player|buffered_output_length|bytes_to_binary|caller_perms|callers|ceil|children|chparent|chparents|chr|clear_property|color|connected_players|connected_seconds|connection_name|connection_option|connection_options|cos|cosh|crypt|ctime|db_disk_size|decode|decode_base64|decode_binary|decompiled_verb_code|delete_property|delete_verb|descendants|disassemble|dump_database|encode|encode_base64|encode_binary|equal|eval|exec|exp|file_chmod|file_close|file_eof|file_flush|file_last_access|file_last_change|file_last_modify|file_list|file_mkdir|file_mode|file_name|file_open|file_openmode|file_read|file_readline|file_readlines|file_remove|file_rename|file_rmdir|file_seek|file_size|file_stat|file_tell|file_type|file_version|file_write|file_writeline|floatstr|floor|flush_input|force_input|function_info|gc_stats|generate_json|getenv|idle_seconds|index|isa|is_clear_property|is_member|is_player|kill_task|length|listappend|listdelete|listen|listeners|listinsert|listset|load_server_options|log|log10|log_cache_stats|mapdelete|mapkeys|mapvalues|match|max|max_object|memory_usage|min|mysql_close|mysql_connect|mysql_connections|mysql_escape_string|mysql_field_names|mysql_next_row|mysql_ping|mysql_query|mysql_set_options|mysql_status|notify|object_bytes|open_network_connection|ord|output_delimiters|parent|parents|parse_json|pass|players|process_id|properties|property_info|queued_tasks|queue_info|raise|random|random_bytes|read|read_http|read_stdin|renumber|reset_max_object|respond_to|resume|rindex|rmatch|run_gc|salt|seconds_left|server_log|server_version|setadd|set_connection_option|set_player_flag|set_property_info|setremove|set_task_local|set_task_perms|set_verb_args|set_verb_code|set_verb_info|shutdown|sin|sinh|sqlite_close|sqlite_column_names|sqlite_delete|sqlite_finalize|sqlite_handles|sqlite_next_row|sqlite_open|sqlite_prepare|sqlite_query|sqlite_status|sqrt|strcmp|string_hash|string_hmac|strsub|strtr|substitute|suspend|switch_player|tan|tanh|task_id|task_local|task_perms|task_stack|ticks_left|time|tofloat|toint|toliteral|tonum|toobj|tostr|trunc|typeof|unlisten|valid|value_bytes|value_hash|value_hmac|verb_args|verb_cache_stats|verb_code|verb_info|verbs)>(\s*\()@=/
syn match mooVerbCall /\v:@<=\h\w*(\s*\()@=/

syn match mooEscape /\v\\(x\x{2}|u\x{4}|[nrte0"\\])/ contained
syn region mooString start=/"/ end=/"/ oneline contains=mooEscape
" Only a lowercase b starts a bytes literal.
syn region mooBytes start=/\C\<b"/ end=/"/ oneline contains=mooEscape

" MOO has no line comments; by convention a statement that is only a string is one.
syn match mooComment /\v^\s*"(\\.|[^"\\])*"\s*;\s*$/ contains=mooTodo
syn region mooComment start=+/\*+ end=+\*/+ contains=mooTodo
syn keyword mooTodo TODO FIXME XXX contained

hi def link mooOperator Operator
hi def link mooInteger Number
hi def link mooFloat Float
hi def link mooObject Constant
hi def link mooSysRef Structure
hi def link mooColour Special
hi def link mooConditional Conditional
hi def link mooRepeat Repeat
hi def link mooException Exception
hi def link mooStatement Statement
hi def link mooKeyword Keyword
hi def link mooError Constant
hi def link mooTypeName Type
hi def link mooVariable Identifier
hi def link mooBuiltin Function
hi def link mooVerbCall Function
hi def link mooEscape SpecialChar
hi def link mooString String
hi def link mooBytes String
hi def link mooComment Comment
hi def link mooTodo Todo

let b:current_syntax = "moo"
let &cpo = s:cpo_save
unlet s:cpo_save
