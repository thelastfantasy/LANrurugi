#!/usr/bin/env perl
# Parses a .pm file with PPI (Perl's own AST library — genuinely correct Perl parsing, unlike a
# hand-rolled regex pass) and dumps the resulting tree as JSON on stdout, for
# `lanrurugi-plugin-converter` (a Rust CLI) to consume and walk into TypeScript.
#
# Runs inside a container (see this directory's Dockerfile) specifically so the host machine
# never needs `perl`/PPI installed — the Rust CLI shells out to `docker run` this image.
use strict;
use warnings;
use PPI;
use JSON::PP;

my $file = $ARGV[0] or die "usage: dump_ast.pl <file.pm>\n";

my $doc = PPI::Document->new($file);
die "PPI failed to parse $file: " . PPI::Document::errstr() . "\n" unless $doc;

sub node_to_hash {
    my ($elem) = @_;
    my %h;
    $h{class} = ref($elem);

    if ($elem->isa('PPI::Token')) {
        $h{content} = $elem->content;
    } elsif ($elem->can('children')) {
        $h{children} = [ map { node_to_hash($_) } $elem->children ];
    }

    if ($elem->isa('PPI::Structure')) {
        my $start  = $elem->start;
        my $finish = $elem->finish;
        $h{start}  = defined($start)  ? $start->content  : undef;
        $h{finish} = defined($finish) ? $finish->content : undef;
    }

    return \%h;
}

my @tree = map { node_to_hash($_) } $doc->children;

print JSON::PP->new->canonical->encode(\@tree);
