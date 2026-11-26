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

if ($path === '/_archivewiki/page-check') {
    header('Cache-Control: no-store');
    header('Content-Type: application/json; charset=utf-8');

    if (($_SERVER['REQUEST_METHOD'] ?? '') !== 'POST') {
        http_response_code(405);
        header('Allow: POST');
        echo json_encode(['error' => 'Use POST to check a page title.']);
        return true;
    }

    $title = $_POST['title'] ?? null;
    if (!is_string($title) || trim($title) === '' || strlen($title) > 512) {
        http_response_code(422);
        echo json_encode(['error' => 'Enter a page title up to 120 characters long.']);
        return true;
    }

    require_once __DIR__ . '/inc/init.php';
    $title = str_replace([':', ';', '/', '\\'], ' ', trim($title));
    $id = cleanID($title);
    if ($id === '' || strlen($id) > 220 || strpos($id, ':') !== false) {
        http_response_code(422);
        echo json_encode(['error' => 'Use a shorter title containing letters or numbers.']);
        return true;
    }

    echo json_encode([
        'id' => $id,
        'exists' => page_exists($id),
    ], JSON_UNESCAPED_UNICODE);
    return true;
}

return false; // let the built-in server handle it normally
