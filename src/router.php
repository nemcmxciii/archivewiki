<?php
// Router for PHP's built-in server. Mirrors DokuWiki's .htaccess protections
// and refuses any request whose Host header is not loopback (DNS-rebinding guard).
$host = strtolower(preg_replace('/:\d+$/', '', $_SERVER['HTTP_HOST'] ?? ''));
$path = rawurldecode(parse_url($_SERVER['REQUEST_URI'] ?? '/', PHP_URL_PATH) ?: '/');

if (!in_array($host, ['127.0.0.1', 'localhost'], true)
    || strpos($path, '..') !== false
    || preg_match('#^/(data|conf|bin|inc|vendor)(/|$)#', $path)
    || preg_match('#/\.#', $path)) {
    http_response_code(403);
    echo 'Forbidden';
    return true;
}
return false; // let the built-in server handle it normally
