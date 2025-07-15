set positional-arguments

default: launch



launch *args='':
  cargo run -- launch $@

launch-with-tracy *args='':
  cargo run --features tracy-profiler -- launch $@
 
