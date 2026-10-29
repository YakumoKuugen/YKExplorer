# Send a line command to a debug-build YKExplorer instance.
# Launch YKExplorer with: $env:YKEXPLORER_DEBUG_PORT='47654'
function Invoke-YKExplorerDebug {
    param(
        [Parameter(Mandatory = $true)][string]$Command,
        [int]$Port = 47654,
        [int]$TimeoutMs = 20000
    )

    $client = [System.Net.Sockets.TcpClient]::new()
    try {
        $client.Connect('127.0.0.1', $Port)
        $client.ReceiveTimeout = $TimeoutMs
        $stream = $client.GetStream()
        $writer = [System.IO.StreamWriter]::new($stream, [System.Text.UTF8Encoding]::new($false))
        $writer.AutoFlush = $true
        $writer.WriteLine($Command)
        $reader = [System.IO.StreamReader]::new($stream, [System.Text.UTF8Encoding]::new($false))
        $line = $reader.ReadLine()
        if ($null -eq $line) { return 'error empty response' }
        return $line
    } finally {
        $client.Dispose()
    }
}
