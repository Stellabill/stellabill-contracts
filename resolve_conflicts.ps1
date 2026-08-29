# Resolve all merge conflict markers, choosing the upstream/main (second) side

$files = Get-ChildItem -Path "C:\Users\ADMIN\Desktop\midea-drips\stellabill-contracts\contracts\subscription_vault\src" -Filter "*.rs" -Recurse

foreach ($file in $files) {
    $text = (Get-Content -Path $file.FullName) -join "`n"
    if ($text -match '<<<<<<< HEAD') {
        Write-Output "Processing: $($file.Name)"
        
        # Pattern: remove <<<<<<< HEAD\n (content) \n=======\n (content) \n>>>>>>> ...\n
        # Keep the "theirs" side (after =======, before >>>>>>>)
        while ($text -match '(?s)<<<<<<< HEAD\n.*?=======\n(.*?)>>>>>>> [^\n]*\n?') {
            $text = $text -replace '(?s)<<<<<<< HEAD\n.*?=======\n(.*?)>>>>>>> [^\n]*\n?', '$1'
        }
        
        Set-Content -Path $file.FullName -Value $text -NoNewline
        Write-Output "  Resolved conflicts in $($file.Name)"
    }
}
