<?php
// Router for PHP's built-in server. Mirrors DokuWiki's .htaccess protections, refuses any request whose Host
// header is not loopback (DNS-rebinding guard), and requires a per-launch secret so that only the ArchiveWiki
// window (not other apps or web pages on this Mac) can use the wiki.
$host = strtolower(preg_replace('/:\d+$/', '', $_SERVER['HTTP_HOST'] ?? ''));
$uri  = $_SERVER['REQUEST_URI'] ?? '/';
$path = rawurldecode(parse_url($uri, PHP_URL_PATH) ?: '/');

if (!in_array($host, ['127.0.0.1', 'localhost'], true)
    || strpos($path, '..') !== false
    || preg_match('#^/(data|conf|bin|inc|vendor)(/|$)#', $path)
    || preg_match('#/\.#', $path)) {
    http_response_code(403);
    echo 'Forbidden';
    return true;
}

$token = getenv('ARCHIVEWIKI_TOKEN');
if ($token) {
    $given  = isset($_GET['aw']) && is_string($_GET['aw']) ? $_GET['aw'] : '';
    $cookie = isset($_COOKIE['aw_session']) && is_string($_COOKIE['aw_session']) ? $_COOKIE['aw_session'] : '';
    if ($given !== '' && hash_equals($token, $given)) {
        // First request from the app: set the session cookie, then reload the same URL without the secret.
        setcookie('aw_session', $token, ['path' => '/', 'httponly' => true, 'samesite' => 'Strict']);
        $q = $_GET;
        unset($q['aw']);
        header('Location: ' . (parse_url($uri, PHP_URL_PATH) ?: '/') . ($q ? '?' . http_build_query($q) : ''));
        return true;
    }
    if ($cookie === '' || !hash_equals($token, $cookie)) {
        http_response_code(403);
        echo 'Forbidden';
        return true;
    }
}
return false; // let the built-in server handle it normally
