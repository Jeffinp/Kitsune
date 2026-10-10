# W41: gzip, zip, unzip and iconv in the Terminal, then "Extrair aqui" in Arquivos. UEFI, 1280x800.
source "$(dirname "$0")/../lib.sh"
wait_first_frame
sleep 2
goto 300 300; click; sleep 0.5
typestr "mkdir t"; key ret; sleep 0.3
typestr "cd t"; key ret; sleep 0.3
typestr "echo ola mundo > a.txt"; key ret; sleep 0.3
typestr "mkdir src"; key ret; sleep 0.3
typestr "echo fn main > src/m.rs"; key ret; sleep 0.3
typestr "zip -r pack src a.txt"; key ret; sleep 1
typestr "unzip -l pack.zip"; key ret; sleep 0.8
typestr "gzip -k a.txt"; key ret; sleep 0.5
typestr "gunzip -c a.txt.gz"; key ret; sleep 0.5
typestr "ls"; key ret; sleep 0.8
shot a-terminal
finish
