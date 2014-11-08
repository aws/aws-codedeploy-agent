# Jamespath [![Version](https://badge.fury.io/rb/jamespath.png)](http://badge.fury.io/rb/jamespath) [![Build Status](https://travis-ci.org/lsegal/jamespath.png?branch=master)](https://travis-ci.org/lsegal/jamespath)

Jamespath is a library that lets you select objects from deeply nested
structures, arrays, hashes, or JSON objects using a simple expression
language.

Think XPath, but for objects.

## Installing

```ruby
$ gem install jamespath
```

Or with Bundler:

```ruby
gem 'jamespath', '~> 1.0'
```

## Usage

To use Jamespath, call the {Jamespath.search} method with an expression
and an object ot search:

```ruby
object = { foo: { bar: ['value1', 'value2', 'value3'] } }
Jamespath.search('foo.bar[0]', object) #=> 'value1'
```

You can also {Jamespath.compile} an expression if you are performing the same
search operation against multiple objects:

```ruby
object1 = { foo: { bar: ['value1', 'value2', 'value3'] } }
object2 = { foo: { bar: ['value4', 'value5', 'value6'] } }

expr = Jamespath.compile('foo.bar[0]')
expr.search(object1) #=> 'value1'
expr.search(object2) #=> 'value4'
```

## Expression Syntax

See the [JMESpath][1] project for more information on the expression syntax.

## License & Acknowledgements 

This library was written by Loren Segal and Trevor Rowe and is licensed under
the MIT license. The implementation is based on the [JMESpath][1] library
written by James Sayerwinnie for the Python programming language.

[1]: http://github.com/boto/jmespath
